//! Fences and screens: what turns an open extrusion (a line with a height and a thickness) into
//! posts with slats, boards, panels, pickets, wire mesh or a solid screen between them, and
//! gates, drawn by `shape_mesh` with stable face names so materials attach: `<id>:posts`,
//! `<id>:infill` (slats, boards, panels, wire), `<id>:rails`, `<id>:gate`.
//!
//! Posts stand at every corner, at both ends and evenly between, at most `post_spacing_m` apart.
//! The infill is laid per span between two posts and does not overlap the posts.
//!
//! A fence follows the curves of its line (`crate::curve`): the line comes as one polyline per
//! control edge (`runs`), and on a curved edge the posts are spread by arc length, never at the
//! tessellation points, with the spans between them straight (closer together where the curve is
//! tight, so a span stays within [`CURVE_TOL_M`] of the curve). A gate needs a straight edge. A gate leaves
//! a gap in the run, a post on each side and a leaf in it.
//!
//! On sloping ground (`Fence::slope`) the posts stand each on the ground at their own foot, always
//! plumb. `stepped` lays every span as one level panel at the higher of its two posts' ground (a
//! gravel board can fill the gap under it), `raked` makes the rails, the top and the bottom follow
//! the line between the two feet, with slats and pickets plumb and cut to it, boards parallel to
//! it and panels as parallelograms. A gate leaf stays level.
//!
//! The wire mesh is a real grid of thin wires (see through, a few hundred triangles a span), not
//! a texture with transparency.

use crate::house::Sink;
use serde::{Deserialize, Serialize};

type P2 = [f32; 2];

/// Faces a fence has, in this order (the plain wall's faces are not there).
pub const FENCE_FACES: [&str; 4] = ["posts", "infill", "rails", "gate"];
const POSTS: usize = 0;
const INFILL: usize = 1;
const RAILS: usize = 2;
const GATE: usize = 3;

/// Gap under the infill and the gate, and how far the posts stand proud of the infill's top.
const CLEARANCE: f32 = 0.05;
const TOP_INSET: f32 = 0.04;
/// Height of a rail board.
const RAIL_H: f32 = 0.05;
/// Height of a picket's pointed tip.
const TIP: f32 = 0.06;
const EPS: f32 = 1e-3;
/// How far a straight span between two posts may lie from the curve it stands on.
pub const CURVE_TOL_M: f32 = 0.05;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FenceKind {
    /// Vertical boards with gaps, on rails.
    #[default]
    Slats,
    /// Horizontal boards with gaps.
    HorizontalSlats,
    /// Closed board panels between the posts.
    Panels,
    /// Low pickets with pointed tops, on rails.
    Picket,
    /// A wire grid between thin posts.
    Mesh,
    /// A solid privacy screen.
    Screen,
}

impl FenceKind {
    /// The count's name in `query.quantities`.
    pub fn item_name(self) -> &'static str {
        match self {
            Self::Slats => "slats",
            Self::HorizontalSlats => "boards",
            Self::Picket => "pickets",
            Self::Panels | Self::Mesh | Self::Screen => "panels",
        }
    }

    /// The depth of the line (the extrusion's `thickness`) a kind starts with.
    pub fn default_thickness(self) -> f32 {
        match self {
            Self::Slats | Self::HorizontalSlats => 0.05,
            Self::Panels => 0.06,
            Self::Picket => 0.04,
            Self::Mesh => 0.02,
            Self::Screen => 0.06,
        }
    }
}

/// How a fence follows ground that is not level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FenceSlope {
    /// The whole fence stands on the lowest ground under its line.
    #[default]
    Level,
    /// Each post stands on its own ground; every span is a level panel, stepping down the slope.
    Stepped,
    /// Each post stands on its own ground; rails and the top and bottom lines run straight between
    /// the feet, slats and pickets stay plumb.
    Raked,
}

impl FenceSlope {
    pub fn name(self) -> &'static str {
        match self {
            Self::Level => "level",
            Self::Stepped => "stepped",
            Self::Raked => "raked",
        }
    }
}

fn is_level(s: &FenceSlope) -> bool {
    *s == FenceSlope::Level
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A gate in the run: a gap of `width_m` centred `offset_m` along the line from its start, with
/// a post on each side and a leaf in it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gate {
    pub offset_m: f32,
    pub width_m: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fence {
    pub kind: FenceKind,
    /// Width of a slat or picket; the height of a horizontal board; the wire's thickness (mesh).
    /// Unused by panels and screens.
    pub slat_width_m: f32,
    /// The gap between slats, boards, pickets or wires.
    pub gap_m: f32,
    /// The most any two neighbouring posts stand apart.
    pub post_spacing_m: f32,
    /// A post's square side.
    pub post_size_m: f32,
    /// Rails behind the infill (slats, pickets, horizontal boards), or framing it (panels, mesh,
    /// screen).
    pub rails: bool,
    pub gates: Vec<Gate>,
    /// How the fence follows sloping ground; only used where there is ground to follow.
    #[serde(skip_serializing_if = "is_level")]
    pub slope: FenceSlope,
    /// Stepped only: a board under each panel, down to the lowest ground under it.
    #[serde(skip_serializing_if = "is_false")]
    pub gravel_board: bool,
}

impl Default for Fence {
    fn default() -> Self {
        Self::new(FenceKind::Slats)
    }
}

impl Fence {
    /// A fence of `kind` with its usual sizes.
    pub fn new(kind: FenceKind) -> Self {
        let (slat_width_m, gap_m, post_spacing_m, post_size_m, rails) = match kind {
            FenceKind::Slats => (0.09, 0.02, 1.8, 0.09, true),
            FenceKind::HorizontalSlats => (0.09, 0.02, 1.8, 0.09, false),
            FenceKind::Panels => (0.0, 0.0, 1.8, 0.1, true),
            FenceKind::Picket => (0.07, 0.05, 1.8, 0.07, true),
            FenceKind::Mesh => (0.012, 0.07, 2.4, 0.06, true),
            FenceKind::Screen => (0.0, 0.0, 2.4, 0.1, false),
        };
        Self {
            kind,
            slat_width_m,
            gap_m,
            post_spacing_m,
            post_size_m,
            rails,
            gates: Vec::new(),
            slope: FenceSlope::Level,
            gravel_board: false,
        }
    }

    /// Whether the fence can be drawn on `line` with this `height` and `thickness`; the reason
    /// if not.
    pub fn check(&self, runs: &[Vec<P2>], height: f32, thickness: f32) -> Result<(), String> {
        let finite = |v: f32, what: &str, lo: f32, hi: f32| {
            if v.is_finite() && (lo..=hi).contains(&v) {
                Ok(())
            } else {
                Err(format!("`{what}` must be a number within {lo}..={hi} m"))
            }
        };
        if runs.is_empty() {
            return Err("a fence line needs at least 2 points".into());
        }
        finite(height, "height", 0.3, 10.0)?;
        finite(thickness, "thickness", 0.005, 1.0)?;
        finite(self.post_spacing_m, "post_spacing_m", 0.5, 6.0)?;
        finite(self.post_size_m, "post_size_m", 0.02, 0.5)?;
        finite(self.gap_m, "gap_m", 0.0, 1.0)?;
        let boards = !matches!(self.kind, FenceKind::Panels | FenceKind::Screen);
        finite(
            self.slat_width_m,
            "slat_width_m",
            if boards { 0.003 } else { 0.0 },
            1.0,
        )?;
        let mut spans: Vec<(f32, f32)> = Vec::new();
        let total: f32 = runs.iter().map(|r| length(r)).sum();
        for (i, g) in self.gates.iter().enumerate() {
            finite(g.width_m, "gate width_m", 0.3, 6.0)?;
            if !g.offset_m.is_finite() {
                return Err(format!("gate {i}: `offset_m` is not a number"));
            }
            let (a, b) = (g.offset_m - g.width_m / 2.0, g.offset_m + g.width_m / 2.0);
            if a < -EPS || b > total + EPS {
                return Err(format!(
                    "gate {i} ({a:.2}..{b:.2} m) lies outside the line (0..{total:.2} m)"
                ));
            }
            // The gate sits in one straight stretch, between two corners.
            let mut start = 0.0;
            let mut inside = false;
            for run in runs {
                let end = start + length(run);
                if run.len() == 2 && a >= start - EPS && b <= end + EPS {
                    inside = true;
                    let post = self.post_size_m;
                    for edge in [a - start, end - b] {
                        if edge > EPS && edge < post {
                            return Err(format!(
                                "gate {i} is closer than a post's width ({post} m) to a corner"
                            ));
                        }
                    }
                }
                start = end;
            }
            if !inside {
                return Err(format!(
                    "gate {i} runs round a corner or along a curve; keep it on one straight edge"
                ));
            }
            if let Some(j) = spans.iter().position(|s| a < s.1 && b > s.0) {
                return Err(format!("gates {j} and {i} overlap"));
            }
            spans.push((a, b));
        }
        let (y0, y1) = self.vertical(0.0, height);
        if y1 - y0 < 0.1 {
            return Err("the fence is too low for its infill".into());
        }
        Ok(())
    }

    /// Bottom and top of the infill for a fence from `lo` to `hi`.
    fn vertical(&self, lo: f32, hi: f32) -> (f32, f32) {
        match self.kind {
            FenceKind::Screen => (lo + 0.02, hi - 0.02),
            FenceKind::Picket => (lo + CLEARANCE, hi),
            _ => (lo + CLEARANCE, hi - TOP_INSET),
        }
    }
}

fn dist(a: P2, b: P2) -> f32 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

fn length(line: &[P2]) -> f32 {
    line.windows(2).map(|w| dist(w[0], w[1])).sum()
}

/// A post: its centre and the direction of the line it stands in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Post {
    pub at: P2,
    pub dir: P2,
}

/// The stretch of one segment between two posts (along the segment, from its start).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub origin: P2,
    pub dir: P2,
    pub a: f32,
    pub b: f32,
    pub gate: bool,
}

/// Where the posts and spans of a fence on `line` lie.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub posts: Vec<Post>,
    pub spans: Vec<Span>,
}

/// Lays the posts and spans out. `runs` must have passed [`Fence::check`].
pub fn layout(f: &Fence, runs: &[Vec<P2>]) -> Layout {
    let mut out = Layout::default();
    let mut start = 0.0;
    for (si, run) in runs.iter().enumerate() {
        let len = length(run);
        if len < EPS {
            start += len;
            continue;
        }
        if run.len() > 2 {
            curved_run(f, run, si > 0, &mut out);
            start += len;
            continue;
        }
        let w = [run[0], run[1]];
        let end = start + len;
        let dir = [(w[1][0] - w[0][0]) / len, (w[1][1] - w[0][1]) / len];
        // Positions along the segment; `true` marks the ones that must stay.
        let n = ((len / f.post_spacing_m) - 1e-4).ceil().max(1.0) as usize;
        let mut at: Vec<(f32, bool)> = (0..=n)
            .map(|k| (len * k as f32 / n as f32, k == 0 || k == n))
            .collect();
        let gates: Vec<(f32, f32)> = f
            .gates
            .iter()
            .map(|g| {
                (
                    g.offset_m - g.width_m / 2.0 - start,
                    g.offset_m + g.width_m / 2.0 - start,
                )
            })
            .filter(|(a, b)| *a >= -EPS && *b <= len + EPS)
            .collect();
        for &(a, b) in &gates {
            at.retain(|&(s, keep)| keep || s < a - f.post_size_m || s > b + f.post_size_m);
            at.extend([(a.max(0.0), true), (b.min(len), true)]);
        }
        at.sort_by(|x, y| x.0.total_cmp(&y.0));
        at.dedup_by(|y, x| (y.0 - x.0).abs() < EPS);
        for (k, &(s, _)) in at.iter().enumerate() {
            // The corner post belongs to the segment before it.
            if k == 0 && si > 0 {
                continue;
            }
            out.posts.push(Post {
                at: [w[0][0] + dir[0] * s, w[0][1] + dir[1] * s],
                dir,
            });
        }
        for pair in at.windows(2) {
            let (a, b) = (pair[0].0, pair[1].0);
            let gate = gates
                .iter()
                .any(|g| (g.0 - a).abs() < EPS && (g.1 - b).abs() < EPS);
            out.spans.push(Span {
                origin: w[0],
                dir,
                a,
                b,
                gate,
            });
        }
        start = end;
    }
    out
}

/// The point `s` metres along a polyline.
fn point_along(run: &[P2], s: f32) -> P2 {
    let mut left = s;
    for w in run.windows(2) {
        let l = dist(w[0], w[1]);
        if left <= l {
            let t = if l > 0.0 { left / l } else { 0.0 };
            return [
                w[0][0] + (w[1][0] - w[0][0]) * t,
                w[0][1] + (w[1][1] - w[0][1]) * t,
            ];
        }
        left -= l;
    }
    *run.last().expect("a run has points")
}

/// How far the straight chords between stations `0, L/n, ..` lie off the polyline `run`.
fn chord_error(run: &[P2], n: usize) -> f32 {
    let total = length(run);
    let stations: Vec<f32> = (0..=n).map(|k| total * k as f32 / n as f32).collect();
    let mut worst = 0.0f32;
    let mut cum = 0.0;
    let mut k = 0;
    for w in run.windows(2) {
        cum += dist(w[0], w[1]);
        while k + 1 < n && stations[k + 1] < cum - EPS {
            k += 1;
        }
        let (a, b) = (
            point_along(run, stations[k]),
            point_along(run, stations[k + 1]),
        );
        let d = [b[0] - a[0], b[1] - a[1]];
        let l = d[0].hypot(d[1]).max(1e-6);
        let off = ((w[1][0] - a[0]) * d[1] - (w[1][1] - a[1]) * d[0]).abs() / l;
        worst = worst.max(off);
    }
    worst
}

/// Posts spread by arc length along a curved edge, and the straight spans between them.
fn curved_run(f: &Fence, run: &[P2], after_corner: bool, out: &mut Layout) {
    let total = length(run);
    let mut n = ((total / f.post_spacing_m) - 1e-4).ceil().max(1.0) as usize;
    while n < 400 && chord_error(run, n) > CURVE_TOL_M {
        n += 1;
    }
    let at: Vec<P2> = (0..=n)
        .map(|k| {
            if k == 0 {
                run[0]
            } else if k == n {
                run[run.len() - 1]
            } else {
                point_along(run, total * k as f32 / n as f32)
            }
        })
        .collect();
    let unit = |a: P2, b: P2| {
        let l = dist(a, b).max(1e-6);
        [(b[0] - a[0]) / l, (b[1] - a[1]) / l]
    };
    for k in 0..=n {
        if k == 0 && after_corner {
            continue;
        }
        let before = (k > 0).then(|| unit(at[k - 1], at[k]));
        let after = (k < n).then(|| unit(at[k], at[k + 1]));
        let dir = match (before, after) {
            (Some(b), Some(a)) => {
                let m = [a[0] + b[0], a[1] + b[1]];
                let l = m[0].hypot(m[1]).max(1e-6);
                [m[0] / l, m[1] / l]
            }
            (Some(d), None) | (None, Some(d)) => d,
            (None, None) => [1.0, 0.0],
        };
        out.posts.push(Post { at: at[k], dir });
    }
    for k in 0..n {
        out.spans.push(Span {
            origin: at[k],
            dir: unit(at[k], at[k + 1]),
            a: 0.0,
            b: dist(at[k], at[k + 1]),
            gate: false,
        });
    }
}

/// How many pieces of a run `avail` long fit `piece` wide with `gap` between, centred.
fn fit(avail: f32, piece: f32, gap: f32) -> usize {
    if avail < piece || piece <= 0.0 {
        return 0;
    }
    (((avail + gap) / (piece + gap) + 1e-4).floor() as usize).max(1)
}

/// The pieces' extent along a run `avail` long, as `(from, to)` pairs from its start.
fn pieces(avail: f32, piece: f32, gap: f32) -> Vec<(f32, f32)> {
    let n = fit(avail, piece, gap);
    if n == 0 {
        return Vec::new();
    }
    let used = n as f32 * piece + (n - 1) as f32 * gap;
    let first = (avail - used) / 2.0;
    (0..n)
        .map(|k| {
            let a = first + k as f32 * (piece + gap);
            (a, a + piece)
        })
        .collect()
}

/// What a fence on a line is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counts {
    pub posts: usize,
    /// Slats, boards, pickets or panels, as `FenceKind::item_name` says; gate leaves not counted.
    pub items: usize,
    pub gates: usize,
}

/// Counts the parts of a fence of `height` on `line`.
pub fn counts(f: &Fence, runs: &[Vec<P2>], height: f32) -> Counts {
    let l = layout(f, runs);
    let (y0, y1) = f.vertical(0.0, height);
    let items = l
        .spans
        .iter()
        .filter(|s| !s.gate)
        .map(|s| {
            let avail = s.b - s.a - f.post_size_m;
            match f.kind {
                FenceKind::Slats | FenceKind::Picket => fit(avail, f.slat_width_m, f.gap_m),
                FenceKind::HorizontalSlats => {
                    if avail > 0.0 {
                        fit(y1 - y0, f.slat_width_m, f.gap_m)
                    } else {
                        0
                    }
                }
                FenceKind::Panels | FenceKind::Mesh | FenceKind::Screen => usize::from(avail > 0.0),
            }
        })
        .sum();
    Counts {
        posts: l.posts.len(),
        items,
        gates: f.gates.len(),
    }
}

/// A box in the line's own frame: `u` along the segment from `o`, `w` across it.
#[derive(Clone, Copy)]
struct Frame {
    o: P2,
    d: P2,
}

impl Frame {
    fn n(self) -> P2 {
        [-self.d[1], self.d[0]]
    }

    fn pt(self, u: f32, w: f32, y: f32) -> [f32; 3] {
        let n = self.n();
        [
            self.o[0] + self.d[0] * u + n[0] * w,
            y,
            self.o[1] + self.d[1] * u + n[1] * w,
        ]
    }

    fn dir3(self) -> [f32; 3] {
        [self.d[0], 0.0, self.d[1]]
    }

    fn n3(self) -> [f32; 3] {
        let n = self.n();
        [n[0], 0.0, n[1]]
    }
}

fn quad(s: &mut dyn Sink, face: usize, v: [[f32; 3]; 4], outward: [f32; 3]) {
    s.tri(face, [v[0], v[1], v[2]], outward);
    s.tri(face, [v[0], v[2], v[3]], outward);
}

fn neg(v: [f32; 3]) -> [f32; 3] {
    v.map(|c| -c)
}

/// A box over `u`, `w` and `y` ranges; `top` leaves the top open (a picket's tip closes it).
fn cuboid(
    s: &mut dyn Sink,
    face: usize,
    fr: Frame,
    u: (f32, f32),
    w: (f32, f32),
    (y0, y1): (f32, f32),
    top: bool,
) {
    prism(s, face, fr, u, w, (y0, y0), (y1, y1), top);
}

/// A post with plumb sides over `u` and `w`, its bottom and top planes running from heights
/// `bottom.0`, `top.0` at `u.0` to `bottom.1`, `top.1` at `u.1`: a box for a level run, a
/// parallelogram (or a slat cut to a raked line) otherwise.
#[allow(clippy::too_many_arguments)]
fn prism(
    s: &mut dyn Sink,
    face: usize,
    fr: Frame,
    (u0, u1): (f32, f32),
    (w0, w1): (f32, f32),
    (b0, b1): (f32, f32),
    (t0, t1): (f32, f32),
    top: bool,
) {
    if u1 - u0 <= 0.0 || w1 - w0 <= 0.0 || t0 - b0 <= 0.0 || t1 - b1 <= 0.0 {
        return;
    }
    let (d, n, up) = (fr.dir3(), fr.n3(), [0.0, 1.0, 0.0]);
    let p = |u, w, y| fr.pt(u, w, y);
    quad(
        s,
        face,
        [p(u0, w0, b0), p(u1, w0, b1), p(u1, w1, b1), p(u0, w1, b0)],
        neg(up),
    );
    if top {
        quad(
            s,
            face,
            [p(u0, w0, t0), p(u1, w0, t1), p(u1, w1, t1), p(u0, w1, t0)],
            up,
        );
    }
    quad(
        s,
        face,
        [p(u0, w0, b0), p(u0, w1, b0), p(u0, w1, t0), p(u0, w0, t0)],
        neg(d),
    );
    quad(
        s,
        face,
        [p(u1, w0, b1), p(u1, w1, b1), p(u1, w1, t1), p(u1, w0, t1)],
        d,
    );
    quad(
        s,
        face,
        [p(u0, w0, b0), p(u1, w0, b1), p(u1, w0, t1), p(u0, w0, t0)],
        neg(n),
    );
    quad(
        s,
        face,
        [p(u0, w1, b0), p(u1, w1, b1), p(u1, w1, t1), p(u0, w1, t0)],
        n,
    );
}

/// The pointed top of a picket: a ridge across its depth at the middle, its apex at `apex`, the
/// shoulders at `shoulder.0` (at `u0`) and `shoulder.1` (at `u1`).
fn tip(
    s: &mut dyn Sink,
    face: usize,
    fr: Frame,
    (u0, u1): (f32, f32),
    (w0, w1): (f32, f32),
    shoulder: (f32, f32),
    apex: f32,
) {
    let um = (u0 + u1) / 2.0;
    let (d, n) = (fr.dir3(), fr.n3());
    let p = |u, w, y| fr.pt(u, w, y);
    let rise = |sign: f32, run: f32, dy: f32| [sign * dy * d[0], run, sign * dy * d[2]];
    quad(
        s,
        face,
        [
            p(u0, w0, shoulder.0),
            p(u0, w1, shoulder.0),
            p(um, w1, apex),
            p(um, w0, apex),
        ],
        rise(-1.0, um - u0, apex - shoulder.0),
    );
    quad(
        s,
        face,
        [
            p(u1, w0, shoulder.1),
            p(u1, w1, shoulder.1),
            p(um, w1, apex),
            p(um, w0, apex),
        ],
        rise(1.0, u1 - um, apex - shoulder.1),
    );
    s.tri(
        face,
        [
            p(u0, w0, shoulder.0),
            p(u1, w0, shoulder.1),
            p(um, w0, apex),
        ],
        neg(n),
    );
    s.tri(
        face,
        [
            p(u0, w1, shoulder.0),
            p(u1, w1, shoulder.1),
            p(um, w1, apex),
        ],
        n,
    );
}

/// The ground the fence follows: its height above the placement's origin at `(x, z)`.
type Ground<'a> = &'a dyn Fn(f32, f32) -> f32;

/// How far a gate leaf hangs above the ground it clears, and above the posts' foot.
const GATE_CLEARANCE: f32 = 0.08;
/// A stepped panel may stand this much above the lowest ground under it, over the usual
/// clearance, before the gap is worth a warning.
const GAP_WARN: f32 = 0.05;

/// Draws a fence of `height` on `line` from `lo` up: posts, infill, rails and gate leaves on the
/// faces of [`FENCE_FACES`]. `line` must have passed [`Fence::check`] with `thickness`.
/// With `ground` (the height of the ground at a point, from the same origin as `lo`) and a
/// `slope` other than level, the fence follows it. Returns what the person should be told.
pub(crate) fn mesh(
    s: &mut dyn Sink,
    f: &Fence,
    runs: &[Vec<P2>],
    thickness: f32,
    lo: f32,
    hi: f32,
    ground: Option<Ground<'_>>,
) -> Vec<String> {
    let l = layout(f, runs);
    let (y0, y1) = f.vertical(lo, hi);
    let t = thickness;
    let follow = ground.filter(|_| f.slope != FenceSlope::Level);
    let g = |p: P2| follow.map_or(0.0, |g| g(p[0], p[1]));
    let (bo, to) = (y0 - lo, y1 - lo);
    let tall = hi - lo;
    let mut warnings = Vec::new();
    // With rails the infill takes the front half of the depth and the rails the back half.
    let (iw, rw) = if f.rails && f.kind != FenceKind::Screen {
        ((-t / 2.0, 0.0), (0.0, t / 2.0))
    } else {
        ((-t / 2.0, t / 2.0), (-t / 2.0, t / 2.0))
    };
    let ps = f.post_size_m / 2.0;
    for post in &l.posts {
        let fr = Frame {
            o: post.at,
            d: post.dir,
        };
        let foot = lo + g(post.at);
        cuboid(
            s,
            POSTS,
            fr,
            (-ps, ps),
            (-ps, ps),
            (foot, foot + tall),
            true,
        );
    }
    let (mut gaps, mut worst_gap) = (0, 0.0f32);
    for sp in &l.spans {
        let fr = Frame {
            o: sp.origin,
            d: sp.dir,
        };
        let (u0, u1) = (sp.a + ps, sp.b - ps);
        if u1 - u0 <= 0.0 {
            continue;
        }
        let avail = u1 - u0;
        let at = |u: f32| [sp.origin[0] + sp.dir[0] * u, sp.origin[1] + sp.dir[1] * u];
        let (ga, gb) = (g(at(sp.a)), g(at(sp.b)));
        let high = ga.max(gb);
        if sp.gate {
            // A gate leaf stays level, on the higher ground of its two posts.
            let up = high;
            gate_leaf(
                s,
                f,
                fr,
                (u0, u1),
                iw,
                rw,
                (y0 + up, y1 + up),
                (lo + up, hi + up),
            );
            if let Some(gr) = follow {
                let rise = (0..=8)
                    .map(|k| {
                        let p = at(u0 + avail * k as f32 / 8.0);
                        gr(p[0], p[1])
                    })
                    .fold(f32::MIN, f32::max)
                    - (high + GATE_CLEARANCE);
                if rise > 1e-3 {
                    let m = at((sp.a + sp.b) / 2.0);
                    warnings.push(format!(
                        "the ground under the gate at ({:.1}, {:.1}) rises {rise:.2} m above its leaf's bottom, so the leaf would not swing; level the ground or move the gate",
                        m[0], m[1]
                    ));
                }
            }
            continue;
        }
        // The line the span's bottom and top follow, at `u` along it.
        let shift = |u: f32| match f.slope {
            FenceSlope::Level => lo,
            FenceSlope::Stepped => lo + high,
            FenceSlope::Raked => {
                let k = if sp.b > sp.a {
                    (u - sp.a) / (sp.b - sp.a)
                } else {
                    0.0
                };
                lo + ga + (gb - ga) * k
            }
        };
        let bl = |u: f32| shift(u) + bo;
        let tl = |u: f32| shift(u) + to;
        let low = (0..=8)
            .map(|k| g(at(u0 + avail * k as f32 / 8.0)))
            .fold(ga.min(gb), f32::min);
        if f.slope == FenceSlope::Stepped {
            let gap = (high + bo) - low;
            if f.gravel_board {
                let (ya, yb) = (lo + low, bl(u0));
                cuboid(s, RAILS, fr, (u0, u1), (-t / 2.0, t / 2.0), (ya, yb), true);
            } else if gap > bo + GAP_WARN {
                gaps += 1;
                worst_gap = worst_gap.max(gap);
            }
        }
        match f.kind {
            FenceKind::Slats => {
                for (a, b) in pieces(avail, f.slat_width_m, f.gap_m) {
                    let (ua, ub) = (u0 + a, u0 + b);
                    prism(
                        s,
                        INFILL,
                        fr,
                        (ua, ub),
                        iw,
                        (bl(ua), bl(ub)),
                        (tl(ua), tl(ub)),
                        true,
                    );
                }
            }
            FenceKind::Picket => {
                for (a, b) in pieces(avail, f.slat_width_m, f.gap_m) {
                    let (ua, ub) = (u0 + a, u0 + b);
                    let shoulder = (tl(ua) - TIP, tl(ub) - TIP);
                    prism(
                        s,
                        INFILL,
                        fr,
                        (ua, ub),
                        iw,
                        (bl(ua), bl(ub)),
                        shoulder,
                        false,
                    );
                    tip(s, INFILL, fr, (ua, ub), iw, shoulder, tl((ua + ub) / 2.0));
                }
            }
            FenceKind::HorizontalSlats => {
                for (a, b) in pieces(to - bo, f.slat_width_m, f.gap_m) {
                    prism(
                        s,
                        INFILL,
                        fr,
                        (u0, u1),
                        iw,
                        (bl(u0) + a, bl(u1) + a),
                        (bl(u0) + b, bl(u1) + b),
                        true,
                    );
                }
            }
            FenceKind::Panels | FenceKind::Screen => {
                prism(
                    s,
                    INFILL,
                    fr,
                    (u0, u1),
                    iw,
                    (bl(u0), bl(u1)),
                    (tl(u0), tl(u1)),
                    true,
                );
            }
            FenceKind::Mesh => wires(
                s,
                INFILL,
                f.slat_width_m,
                f.gap_m,
                fr,
                (u0, u1),
                iw,
                rw,
                (&bl, &tl),
            ),
        }
        if f.rails {
            for (ya, yb) in rail_heights(f, 0.0, to - bo) {
                prism(
                    s,
                    RAILS,
                    fr,
                    (u0, u1),
                    rw,
                    (bl(u0) + ya, bl(u1) + ya),
                    (bl(u0) + yb, bl(u1) + yb),
                    true,
                );
            }
        }
    }
    if gaps > 0 {
        warnings.push(format!(
            "{gaps} stepped panel{} stand{} up to {worst_gap:.2} m above the ground at the low end, a gap an animal or a child gets through; set `gravel_board` to close it or use `slope: raked`",
            if gaps == 1 { "" } else { "s" },
            if gaps == 1 { "s" } else { "" },
        ));
    }
    warnings
}

/// A grid of thin wires: verticals in the front of the depth, horizontals behind them. `lines` are
/// the bottom and the top of the grid at `u`; the wires stay plumb and the horizontals follow them.
#[allow(clippy::too_many_arguments)]
fn wires(
    s: &mut dyn Sink,
    face: usize,
    wire: f32,
    gap: f32,
    fr: Frame,
    (u0, u1): (f32, f32),
    iw: (f32, f32),
    rw: (f32, f32),
    (bl, tl): (&dyn Fn(f32) -> f32, &dyn Fn(f32) -> f32),
) {
    let wx = (iw.1 - wire.min(iw.1 - iw.0), iw.1);
    for (a, b) in pieces(u1 - u0, wire, gap) {
        let (ua, ub) = (u0 + a, u0 + b);
        prism(
            s,
            face,
            fr,
            (ua, ub),
            wx,
            (bl(ua), bl(ub)),
            (tl(ua), tl(ub)),
            true,
        );
    }
    let wz = (rw.0, rw.0 + wire.min(rw.1 - rw.0));
    for (a, b) in pieces(tl(u0) - bl(u0), wire, gap) {
        prism(
            s,
            face,
            fr,
            (u0, u1),
            wz,
            (bl(u0) + a, bl(u1) + a),
            (bl(u0) + b, bl(u1) + b),
            true,
        );
    }
}

/// The rail boards' bottoms and tops, from a bottom at `y0` and a top at `y1`.
fn rail_heights(f: &Fence, y0: f32, y1: f32) -> Vec<(f32, f32)> {
    match f.kind {
        FenceKind::Slats | FenceKind::Picket | FenceKind::HorizontalSlats => {
            let h = y1 - y0;
            [0.2, 0.8]
                .map(|k| {
                    let c = y0 + h * k;
                    (c - RAIL_H / 2.0, c + RAIL_H / 2.0)
                })
                .to_vec()
        }
        FenceKind::Panels | FenceKind::Mesh | FenceKind::Screen => {
            vec![(y0, y0 + RAIL_H), (y1 - RAIL_H, y1)]
        }
    }
}

/// The leaf of a gate: a board panel with two rails, a little clear of the posts and the ground.
#[allow(clippy::too_many_arguments)]
fn gate_leaf(
    s: &mut dyn Sink,
    f: &Fence,
    fr: Frame,
    (u0, u1): (f32, f32),
    iw: (f32, f32),
    rw: (f32, f32),
    (y0, y1): (f32, f32),
    (lo, hi): (f32, f32),
) {
    let (ua, ub) = (u0 + 0.02, u1 - 0.02);
    let (ya, yb) = (lo + GATE_CLEARANCE, (hi - GATE_CLEARANCE).min(y1));
    let ya = ya.max(y0.min(lo + GATE_CLEARANCE));
    if f.kind == FenceKind::Mesh {
        // See through, like the fence: a wire grid between a top and a bottom bar.
        wires(
            s,
            GATE,
            f.slat_width_m,
            f.gap_m,
            fr,
            (ua, ub),
            iw,
            rw,
            (&|_| ya, &|_| yb),
        );
        for (a, b) in [(ya, ya + RAIL_H), (yb - RAIL_H, yb)] {
            cuboid(
                s,
                GATE,
                fr,
                (ua, ub),
                (-RAIL_H / 4.0, RAIL_H / 4.0),
                (a, b),
                true,
            );
        }
        return;
    }
    cuboid(s, GATE, fr, (ua, ub), iw, (ya, yb), true);
    let h = yb - ya;
    for k in [0.18, 0.82] {
        let c = ya + h * k;
        cuboid(
            s,
            GATE,
            fr,
            (ua, ub),
            rw,
            (c - RAIL_H / 2.0, c + RAIL_H / 2.0),
            true,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::{Extrusion, Shape, shape_mesh};

    /// One run per segment of `line`.
    fn straight(line: &[P2]) -> Vec<Vec<P2>> {
        line.windows(2).map(|w| w.to_vec()).collect()
    }

    fn fence_shape(f: Fence, line: Vec<P2>, height: f32) -> Shape {
        Shape::Extrusion(Extrusion {
            id: "S1".into(),
            name: "Fence".into(),
            layer: "shapes".into(),
            points: line,
            curves: Default::default(),
            smooth: false,
            closed: false,
            base: 0.0,
            height,
            thickness: f.kind.default_thickness(),
            extensions: Default::default(),
            house: None,
            fence: Some(f),
            stairs: None,
            raised_bed: None,
            furniture: None,
        })
    }

    fn xs(l: &Layout) -> Vec<(f32, f32)> {
        l.posts.iter().map(|p| (p.at[0], p.at[1])).collect()
    }

    #[test]
    fn posts_stand_at_corners_ends_and_at_most_the_spacing_apart() {
        let f = Fence::new(FenceKind::Slats);
        // 4 m east, then 2 m north: a corner at (4, 0).
        let line = [[0.0, 0.0], [4.0, 0.0], [4.0, 2.0]];
        f.check(&straight(&line), 1.8, 0.05).unwrap();
        let l = layout(&f, &straight(&line));
        let posts = xs(&l);
        // 4 m at 1.8 -> 3 pieces; 2 m at 1.8 -> 2 pieces: 4 + 2 posts, the corner once.
        assert_eq!(posts.len(), 6, "{posts:?}");
        assert!(posts.contains(&(0.0, 0.0)));
        assert!(posts.iter().any(|p| (p.0 - 4.0).abs() < 1e-5 && p.1 == 0.0));
        assert!(
            posts
                .iter()
                .any(|p| (p.0 - 4.0).abs() < 1e-5 && (p.1 - 2.0).abs() < 1e-5)
        );
        for s in &l.spans {
            assert!(s.b - s.a <= 1.8 + 1e-4);
        }
        assert_eq!(l.spans.len(), 5);
    }

    #[test]
    fn posts_on_an_arc_are_spread_by_arc_length_not_at_tessellation_points() {
        use crate::curve::Curve;
        let f = Fence::new(FenceKind::Slats);
        // A semicircle of radius 3 over a 6 m chord: arc length 3 pi = 9.42 m.
        let mut shape = fence_shape(f.clone(), vec![[0.0, 0.0], [6.0, 0.0]], 1.8);
        let Shape::Extrusion(e) = &mut shape else {
            unreachable!()
        };
        e.curves.insert(0, Curve::Arc { sagitta_m: 3.0 });
        let runs = shape.tessellate().runs();
        assert_eq!(runs.len(), 1);
        assert!(runs[0].len() > 20, "the arc is tessellated finely");
        f.check(&runs, 1.8, 0.05).unwrap();
        let l = layout(&f, &runs);
        assert!(l.posts.len() < runs[0].len() / 2, "not a post per point");
        // Posts lie on the circle and are evenly spaced along it.
        let angle = |p: P2| ((p[0] - 3.0) / 3.0).clamp(-1.0, 1.0).acos();
        for p in &l.posts {
            assert!((dist(p.at, [3.0, 0.0]) - 3.0).abs() < 1e-3);
        }
        let steps: Vec<f32> = l
            .posts
            .windows(2)
            .map(|w| (angle(w[1].at) - angle(w[0].at)).abs() * 3.0)
            .collect();
        let first = steps[0];
        assert!(steps.iter().all(|s| (s - first).abs() < 0.02), "{steps:?}");
        assert!(first * 1.0 <= 1.8 + 1e-3);
        assert_eq!(l.spans.len(), l.posts.len() - 1);
        // Each straight span stays near the curve.
        for sp in &l.spans {
            let mid = [
                sp.origin[0] + sp.dir[0] * (sp.b / 2.0),
                sp.origin[1] + sp.dir[1] * (sp.b / 2.0),
            ];
            assert!((dist(mid, [3.0, 0.0]) - 3.0).abs() <= CURVE_TOL_M + 1e-3);
        }
        // It meshes, with the usual faces, and a gate on the curve is refused.
        shape_mesh(&shape).unwrap();
        let mut g = f;
        g.gates.push(Gate {
            offset_m: 4.0,
            width_m: 1.0,
        });
        assert!(g.check(&runs, 1.8, 0.05).unwrap_err().contains("straight"));
    }

    #[test]
    fn a_gate_leaves_a_gap_with_a_post_on_each_side() {
        let mut f = Fence::new(FenceKind::Slats);
        f.gates.push(Gate {
            offset_m: 5.0,
            width_m: 1.2,
        });
        let line = [[0.0, 0.0], [10.0, 0.0]];
        f.check(&straight(&line), 1.8, 0.05).unwrap();
        let l = layout(&f, &straight(&line));
        let x: Vec<f32> = l.posts.iter().map(|p| p.at[0]).collect();
        assert!(x.iter().any(|v| (v - 4.4).abs() < 1e-4), "{x:?}");
        assert!(x.iter().any(|v| (v - 5.6).abs() < 1e-4), "{x:?}");
        assert!(!x.iter().any(|v| *v > 4.45 && *v < 5.55), "{x:?}");
        let gates: Vec<&Span> = l.spans.iter().filter(|s| s.gate).collect();
        assert_eq!(gates.len(), 1);
        assert!((gates[0].a - 4.4).abs() < 1e-4 && (gates[0].b - 5.6).abs() < 1e-4);
        // No infill spans the gap.
        assert!(
            l.spans
                .iter()
                .filter(|s| !s.gate)
                .all(|s| s.b <= 4.4 + 1e-4 || s.a >= 5.6 - 1e-4)
        );
        assert_eq!(counts(&f, &straight(&line), 1.8).gates, 1);
    }

    #[test]
    fn slats_are_counted_for_a_known_length() {
        let f = Fence::new(FenceKind::Slats);
        let line = [[0.0, 0.0], [2.0, 0.0]];
        // Two spans of 1.0 m; 0.9 m clear of the posts; (0.9 + 0.02) / 0.11 = 8 slats each.
        let c = counts(&f, &straight(&line), 1.8);
        assert_eq!((c.posts, c.items), (3, 16));
    }

    #[test]
    fn bad_fences_are_refused_with_a_reason() {
        let line = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0]];
        let mut f = Fence::new(FenceKind::Slats);
        f.gates.push(Gate {
            offset_m: 9.0,
            width_m: 1.0,
        });
        assert!(
            f.check(&straight(&line), 1.8, 0.05)
                .unwrap_err()
                .contains("outside")
        );
        f.gates[0].offset_m = 4.0;
        assert!(
            f.check(&straight(&line), 1.8, 0.05)
                .unwrap_err()
                .contains("corner")
        );
        f.gates.clear();
        f.post_spacing_m = 0.1;
        assert!(
            f.check(&straight(&line), 1.8, 0.05)
                .unwrap_err()
                .contains("post_spacing_m")
        );
        f.post_spacing_m = f32::NAN;
        assert!(f.check(&straight(&line), 1.8, 0.05).is_err());
        let g = Fence::new(FenceKind::Slats);
        assert!(g.check(&straight(&line), f32::INFINITY, 0.05).is_err());
        assert!(g.check(&straight(&line[..1]), 1.8, 0.05).is_err());
    }

    #[test]
    fn every_kind_meshes_finite_with_all_face_names_inside_the_line_and_height() {
        for kind in [
            FenceKind::Slats,
            FenceKind::HorizontalSlats,
            FenceKind::Panels,
            FenceKind::Picket,
            FenceKind::Mesh,
            FenceKind::Screen,
        ] {
            let mut f = Fence::new(kind);
            f.gates.push(Gate {
                offset_m: 3.0,
                width_m: 1.0,
            });
            let shape = fence_shape(f.clone(), vec![[1.0, 2.0], [8.0, 2.0], [8.0, 6.0]], 1.8);
            let m = shape_mesh(&shape).unwrap();
            let names: Vec<&str> = m.faces.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(
                names,
                ["S1:posts", "S1:infill", "S1:rails", "S1:gate"],
                "{kind:?}"
            );
            assert!(!m.triangles.is_empty());
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &m.positions {
                for k in 0..3 {
                    assert!(p[k].is_finite());
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            let slack = 0.1 + 1e-4; // half a post at the ends and corner
            assert!(
                lo[0] > 1.0 - slack && hi[0] < 8.0 + slack,
                "{kind:?} {lo:?} {hi:?}"
            );
            assert!(lo[2] > 2.0 - slack && hi[2] < 6.0 + slack);
            assert!((lo[0] - 0.95).abs() < 0.06 && (hi[0] - 8.05).abs() < 0.06);
            assert!(
                lo[1] >= -1e-5 && (hi[1] - 1.8).abs() < 1e-4,
                "{kind:?} {lo:?} {hi:?}"
            );
            for face in ["S1:posts", "S1:infill", "S1:gate"] {
                let i = m.faces.iter().position(|f| f.name == face).unwrap() as u32;
                assert!(m.triangle_faces.contains(&i), "{kind:?} lacks {face}");
            }
        }
    }

    #[test]
    fn a_fence_without_a_gate_has_an_empty_gate_face_and_a_reasonable_size() {
        let shape = fence_shape(
            Fence::new(FenceKind::Mesh),
            vec![[0.0, 0.0], [30.0, 0.0]],
            1.8,
        );
        let m = shape_mesh(&shape).unwrap();
        assert!(m.triangles.len() < 40_000, "{}", m.triangles.len());
        let gate = m.faces.iter().position(|f| f.name == "S1:gate").unwrap() as u32;
        assert!(!m.triangle_faces.contains(&gate));
    }

    // --- Sloping ground ---------------------------------------------------------------------

    use crate::ImportedModel;
    use crate::shape::shape_mesh_on;

    /// Ground rising 1 m in 6 along x, then flat from x = 3.6 (0.6 m up).
    fn hill(x: f32, _z: f32) -> Option<f32> {
        Some((x / 6.0).min(0.6))
    }

    fn sloped(kind: FenceKind, slope: FenceSlope, gates: &[(f32, f32)]) -> (Fence, ImportedModel) {
        let mut f = Fence::new(kind);
        f.slope = slope;
        f.gates = gates
            .iter()
            .map(|&(offset_m, width_m)| Gate { offset_m, width_m })
            .collect();
        let shape = fence_shape(f.clone(), vec![[0.0, 0.0], [9.0, 0.0]], 1.8);
        let m = shape_mesh_on(&shape, &hill).unwrap();
        (f, m)
    }

    /// The vertices of the triangles of face `<id>:<name>`.
    fn verts(m: &ImportedModel, name: &str) -> Vec<[f32; 3]> {
        let face = m
            .faces
            .iter()
            .position(|f| f.name == format!("S1:{name}"))
            .unwrap() as u32;
        m.triangles
            .iter()
            .zip(&m.triangle_faces)
            .filter(|(_, f)| **f == face)
            .flat_map(|(t, _)| t.iter().map(|&i| m.positions[i as usize]))
            .collect()
    }

    fn posts_of(f: &Fence) -> Layout {
        layout(f, &straight(&[[0.0, 0.0], [9.0, 0.0]]))
    }

    #[test]
    fn stepped_posts_stand_on_their_own_ground_and_reach_the_fence_height_above_it() {
        let (f, m) = sloped(FenceKind::Slats, FenceSlope::Stepped, &[]);
        let posts = verts(&m, "posts");
        let l = posts_of(&f);
        assert_eq!(l.posts.len(), 6);
        for p in &l.posts {
            let near: Vec<f32> = posts
                .iter()
                .filter(|v| (v[0] - p.at[0]).abs() <= 0.05 + 1e-4)
                .map(|v| v[1])
                .collect();
            let g = hill(p.at[0], 0.0).unwrap();
            let (lo, hi) = near
                .iter()
                .fold((f32::MAX, f32::MIN), |a, &y| (a.0.min(y), a.1.max(y)));
            assert!((lo - g).abs() < 1e-4, "foot {lo} on ground {g}");
            assert!((hi - (g + 1.8)).abs() < 1e-4, "top {hi}");
        }
    }

    #[test]
    fn each_stepped_panel_is_level_and_above_both_feet() {
        let (f, m) = sloped(FenceKind::Panels, FenceSlope::Stepped, &[]);
        let infill = verts(&m, "infill");
        for sp in &posts_of(&f).spans {
            let ga = hill(sp.origin[0] + sp.a, 0.0).unwrap();
            let gb = hill(sp.origin[0] + sp.b, 0.0).unwrap();
            let ys: Vec<f32> = infill
                .iter()
                .filter(|v| v[0] > sp.origin[0] + sp.a && v[0] < sp.origin[0] + sp.b)
                .map(|v| v[1])
                .collect();
            let (lo, hi) = ys
                .iter()
                .fold((f32::MAX, f32::MIN), |a, &y| (a.0.min(y), a.1.max(y)));
            // Level: only two heights, the bottom and the top.
            assert!(
                ys.iter()
                    .all(|y| (y - lo).abs() < 1e-5 || (y - hi).abs() < 1e-5)
            );
            assert!(
                (lo - (ga.max(gb) + CLEARANCE)).abs() < 1e-4,
                "{lo} {ga} {gb}"
            );
            assert!(lo > ga && lo > gb);
            assert!((hi - lo - (1.8 - CLEARANCE - TOP_INSET)).abs() < 1e-4);
        }
    }

    #[test]
    fn a_stepped_gap_warns_and_a_gravel_board_closes_it() {
        let (_, m) = sloped(FenceKind::Slats, FenceSlope::Stepped, &[]);
        assert!(
            m.warnings.iter().any(|w| w.contains("gravel_board")),
            "{:?}",
            m.warnings
        );
        let shape = {
            let mut f = Fence::new(FenceKind::Slats);
            f.slope = FenceSlope::Stepped;
            f.gravel_board = true;
            fence_shape(f, vec![[0.0, 0.0], [9.0, 0.0]], 1.8)
        };
        let m = shape_mesh_on(&shape, &hill).unwrap();
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        // The board reaches down to the lowest ground under each panel: the first panel's board
        // starts at the ground at its low post (0).
        let rails = verts(&m, "rails");
        let first: Vec<f32> = rails
            .iter()
            .filter(|v| v[0] > 0.0 && v[0] < 1.8)
            .map(|v| v[1])
            .collect();
        assert!(first.iter().any(|y| y.abs() < 1e-4), "{first:?}");
        // Raked and level fences leave no gap to warn about.
        let (_, raked) = sloped(FenceKind::Slats, FenceSlope::Raked, &[]);
        assert!(raked.warnings.is_empty());
    }

    /// The height the ground line between the posts has at `x` (the hill is straight between
    /// posts: it bends at the post at 3.6).
    fn line(x: f32) -> f32 {
        hill(x, 0.0).unwrap()
    }

    #[test]
    fn raked_rails_follow_the_line_between_the_feet() {
        let (f, m) = sloped(FenceKind::Slats, FenceSlope::Raked, &[]);
        let (y0, y1) = f.vertical(0.0, 1.8);
        let h = y1 - y0;
        let expect: Vec<f32> = [0.2, 0.8]
            .iter()
            .flat_map(|k| [y0 + h * k - RAIL_H / 2.0, y0 + h * k + RAIL_H / 2.0])
            .collect();
        let rails = verts(&m, "rails");
        assert!(!rails.is_empty());
        for v in &rails {
            let rel = v[1] - line(v[0]);
            assert!(
                expect.iter().any(|e| (e - rel).abs() < 1e-4),
                "rail vertex {v:?} is {rel} above the line"
            );
        }
        // And the rails really climb: the first span's rail is higher at its far end.
        let at = |x: f32| {
            rails
                .iter()
                .filter(|v| (v[0] - x).abs() < 1e-4)
                .map(|v| v[1])
                .fold(f32::MAX, f32::min)
        };
        let (a, b) = (0.045, 1.755);
        assert!((at(b) - at(a) - (line(b) - line(a))).abs() < 1e-4);
    }

    #[test]
    fn raked_slats_are_plumb_with_their_tops_on_the_raked_line() {
        let (f, m) = sloped(FenceKind::Slats, FenceSlope::Raked, &[]);
        let (y0, y1) = f.vertical(0.0, 1.8);
        let infill = verts(&m, "infill");
        assert!(!infill.is_empty());
        // Every slat edge is a vertical pair: the bottom and the top sit on the same (x, z).
        let mut columns: std::collections::BTreeMap<(i32, i32), Vec<f32>> = Default::default();
        for v in &infill {
            let key = ((v[0] * 1e4).round() as i32, (v[2] * 1e4).round() as i32);
            columns.entry(key).or_default().push(v[1]);
        }
        for (key, ys) in &columns {
            let x = key.0 as f32 / 1e4;
            let (lo, hi) = ys
                .iter()
                .fold((f32::MAX, f32::MIN), |a, &y| (a.0.min(y), a.1.max(y)));
            assert!((lo - (line(x) + y0)).abs() < 1e-3, "bottom at {x}");
            assert!((hi - (line(x) + y1)).abs() < 1e-3, "top at {x}");
            assert!(
                ys.iter()
                    .all(|y| (y - lo).abs() < 1e-4 || (y - hi).abs() < 1e-4)
            );
        }
    }

    #[test]
    fn raked_pickets_have_their_tips_on_the_line_and_each_one_its_own_height() {
        let (f, m) = sloped(FenceKind::Picket, FenceSlope::Raked, &[]);
        let (_, y1) = f.vertical(0.0, 1.8);
        let infill = verts(&m, "infill");
        assert!(!infill.is_empty());
        // The highest point of the first picket lies on the line at its middle.
        let x0 = infill.iter().map(|v| v[0]).fold(f32::MAX, f32::min);
        let first: Vec<&[f32; 3]> = infill
            .iter()
            .filter(|v| v[0] <= x0 + f.slat_width_m + 1e-4)
            .collect();
        let apex = first.iter().map(|v| v[1]).fold(f32::MIN, f32::max);
        let mid = x0 + f.slat_width_m / 2.0;
        assert!((apex - (line(mid) + y1)).abs() < 1e-3, "{apex}");
        // Each picket is cut to its own place on the line: the tips are not all level.
        let top = |x: f32| {
            infill
                .iter()
                .filter(|v| (v[0] - x).abs() < f.slat_width_m)
                .map(|v| v[1])
                .fold(f32::MIN, f32::max)
        };
        assert!(top(1.5) > top(0.3) + 0.1);
    }

    #[test]
    fn raked_boards_run_parallel_to_the_rake_and_the_mesh_is_a_sheared_grid() {
        // Straight ground, so the line between any two posts is the ground itself.
        let slope = |x: f32, _z: f32| Some(x / 6.0);
        for kind in [
            FenceKind::HorizontalSlats,
            FenceKind::Mesh,
            FenceKind::Panels,
        ] {
            let mut f = Fence::new(kind);
            f.slope = FenceSlope::Raked;
            let shape = fence_shape(f, vec![[0.0, 0.0], [9.0, 0.0]], 1.8);
            let m = shape_mesh_on(&shape, &slope).unwrap();
            let infill = verts(&m, "infill");
            assert!(!infill.is_empty(), "{kind:?}");
            // Everything is cut to the rake: the height above the line takes few values.
            let mut above: Vec<f32> = infill.iter().map(|v| v[1] - v[0] / 6.0).collect();
            above.sort_by(f32::total_cmp);
            above.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
            let cap = if kind == FenceKind::Panels { 2 } else { 80 };
            assert!(above.len() <= cap, "{kind:?}: {} offsets", above.len());
        }
    }

    #[test]
    fn the_level_mode_ignores_the_ground() {
        for kind in [FenceKind::Slats, FenceKind::Picket, FenceKind::Mesh] {
            let (_, on) = sloped(kind, FenceSlope::Level, &[(4.5, 1.2)]);
            let mut f = Fence::new(kind);
            f.gates = vec![Gate {
                offset_m: 4.5,
                width_m: 1.2,
            }];
            let flat = shape_mesh(&fence_shape(f, vec![[0.0, 0.0], [9.0, 0.0]], 1.8)).unwrap();
            assert_eq!(on.positions, flat.positions, "{kind:?}");
            assert_eq!(on.triangles, flat.triangles, "{kind:?}");
            assert!(on.warnings.is_empty());
        }
    }

    #[test]
    fn a_gate_stays_level_on_the_higher_post_and_warns_when_the_ground_rises_under_it() {
        // On the slope between 3.9 and 5.1 m: the ground is 0.65 and 0.6 (flat from 3.6).
        for slope in [FenceSlope::Stepped, FenceSlope::Raked] {
            let (f, m) = sloped(FenceKind::Slats, slope, &[(1.0, 1.2)]);
            let gate = verts(&m, "gate");
            assert!(!gate.is_empty());
            let l = posts_of(&f);
            // Posts either side keep their own feet.
            let posts = verts(&m, "posts");
            for p in l
                .posts
                .iter()
                .filter(|p| (p.at[0] - 0.4).abs() < 1e-3 || (p.at[0] - 1.6).abs() < 1e-3)
            {
                let foot = posts
                    .iter()
                    .filter(|v| (v[0] - p.at[0]).abs() <= 0.05 + 1e-4)
                    .map(|v| v[1])
                    .fold(f32::MAX, f32::min);
                assert!((foot - line(p.at[0])).abs() < 1e-4);
            }
            // The leaf is level: only a few heights, its bottom 0.08 above the higher post's foot.
            let high = line(1.6);
            let bottom = gate.iter().map(|v| v[1]).fold(f32::MAX, f32::min);
            assert!((bottom - (high + GATE_CLEARANCE)).abs() < 1e-4, "{bottom}");
            assert!(
                !m.warnings.iter().any(|w| w.contains("swing")),
                "{:?}",
                m.warnings
            );
        }
        // A ridge under the gate, higher than the leaf's bottom.
        let ridge = |x: f32, _z: f32| Some(if (4.2..4.8).contains(&x) { 0.4 } else { 0.0 });
        let mut f = Fence::new(FenceKind::Slats);
        f.slope = FenceSlope::Raked;
        f.gates = vec![Gate {
            offset_m: 4.5,
            width_m: 1.2,
        }];
        let shape = fence_shape(f.clone(), vec![[0.0, 0.0], [9.0, 0.0]], 1.8);
        let m = shape_mesh_on(&shape, &ridge).unwrap();
        assert!(
            m.warnings.iter().any(|w| w.contains("would not swing")),
            "{:?}",
            m.warnings
        );
        // Level mode says nothing.
        f.slope = FenceSlope::Level;
        let m = shape_mesh_on(&fence_shape(f, vec![[0.0, 0.0], [9.0, 0.0]], 1.8), &ridge).unwrap();
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn a_sloped_fence_without_ground_is_the_level_fence() {
        let mut f = Fence::new(FenceKind::Slats);
        let level = shape_mesh(&fence_shape(f.clone(), vec![[0.0, 0.0], [9.0, 0.0]], 1.8)).unwrap();
        for slope in [FenceSlope::Stepped, FenceSlope::Raked] {
            f.slope = slope;
            let m = shape_mesh(&fence_shape(f.clone(), vec![[0.0, 0.0], [9.0, 0.0]], 1.8)).unwrap();
            assert_eq!(m.positions, level.positions);
        }
    }

    #[test]
    fn the_slope_survives_a_round_trip_and_old_files_without_it_are_level() {
        let mut f = Fence::new(FenceKind::Panels);
        f.slope = FenceSlope::Raked;
        let json = serde_json::to_value(&f).unwrap();
        assert_eq!(json["slope"], "raked");
        assert_eq!(serde_json::from_value::<Fence>(json).unwrap(), f);
        let plain = serde_json::to_value(Fence::new(FenceKind::Slats)).unwrap();
        assert!(plain.get("slope").is_none() && plain.get("gravel_board").is_none());
        let old: Fence = serde_json::from_value(serde_json::json!({"kind": "slats"})).unwrap();
        assert_eq!((old.slope, old.gravel_board), (FenceSlope::Level, false));
        let mut g = Fence::new(FenceKind::Slats);
        g.slope = FenceSlope::Stepped;
        g.gravel_board = true;
        let back: Fence = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(back, g);
    }
}
