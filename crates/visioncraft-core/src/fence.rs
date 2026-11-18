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
    (u0, u1): (f32, f32),
    (w0, w1): (f32, f32),
    (y0, y1): (f32, f32),
    top: bool,
) {
    if u1 - u0 <= 0.0 || w1 - w0 <= 0.0 || y1 - y0 <= 0.0 {
        return;
    }
    let (d, n, up) = (fr.dir3(), fr.n3(), [0.0, 1.0, 0.0]);
    let p = |u, w, y| fr.pt(u, w, y);
    quad(
        s,
        face,
        [p(u0, w0, y0), p(u1, w0, y0), p(u1, w1, y0), p(u0, w1, y0)],
        neg(up),
    );
    if top {
        quad(
            s,
            face,
            [p(u0, w0, y1), p(u1, w0, y1), p(u1, w1, y1), p(u0, w1, y1)],
            up,
        );
    }
    quad(
        s,
        face,
        [p(u0, w0, y0), p(u0, w1, y0), p(u0, w1, y1), p(u0, w0, y1)],
        neg(d),
    );
    quad(
        s,
        face,
        [p(u1, w0, y0), p(u1, w1, y0), p(u1, w1, y1), p(u1, w0, y1)],
        d,
    );
    quad(
        s,
        face,
        [p(u0, w0, y0), p(u1, w0, y0), p(u1, w0, y1), p(u0, w0, y1)],
        neg(n),
    );
    quad(
        s,
        face,
        [p(u0, w1, y0), p(u1, w1, y0), p(u1, w1, y1), p(u0, w1, y1)],
        n,
    );
}

/// The pointed top of a picket: a ridge across its depth, `TIP` above `y`.
fn tip(
    s: &mut dyn Sink,
    face: usize,
    fr: Frame,
    (u0, u1): (f32, f32),
    (w0, w1): (f32, f32),
    y: f32,
) {
    let (um, yt) = ((u0 + u1) / 2.0, y + TIP);
    let (d, n) = (fr.dir3(), fr.n3());
    let p = |u, w, y| fr.pt(u, w, y);
    let rise = |sign: f32, run: f32| [sign * TIP * d[0], run, sign * TIP * d[2]];
    quad(
        s,
        face,
        [p(u0, w0, y), p(u0, w1, y), p(um, w1, yt), p(um, w0, yt)],
        rise(-1.0, um - u0),
    );
    quad(
        s,
        face,
        [p(u1, w0, y), p(u1, w1, y), p(um, w1, yt), p(um, w0, yt)],
        rise(1.0, u1 - um),
    );
    s.tri(face, [p(u0, w0, y), p(u1, w0, y), p(um, w0, yt)], neg(n));
    s.tri(face, [p(u0, w1, y), p(u1, w1, y), p(um, w1, yt)], n);
}

/// Draws a fence of `height` on `line` from `lo` up: posts, infill, rails and gate leaves on the
/// faces of [`FENCE_FACES`]. `line` must have passed [`Fence::check`] with `thickness`.
pub(crate) fn mesh(
    s: &mut dyn Sink,
    f: &Fence,
    runs: &[Vec<P2>],
    thickness: f32,
    lo: f32,
    hi: f32,
) {
    let l = layout(f, runs);
    let (y0, y1) = f.vertical(lo, hi);
    let t = thickness;
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
        cuboid(s, POSTS, fr, (-ps, ps), (-ps, ps), (lo, hi), true);
    }
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
        if sp.gate {
            gate_leaf(s, f, fr, (u0, u1), iw, rw, (y0, y1), (lo, hi));
            continue;
        }
        match f.kind {
            FenceKind::Slats => {
                for (a, b) in pieces(avail, f.slat_width_m, f.gap_m) {
                    cuboid(s, INFILL, fr, (u0 + a, u0 + b), iw, (y0, y1), true);
                }
            }
            FenceKind::Picket => {
                for (a, b) in pieces(avail, f.slat_width_m, f.gap_m) {
                    let (ua, ub) = (u0 + a, u0 + b);
                    cuboid(s, INFILL, fr, (ua, ub), iw, (y0, y1 - TIP), false);
                    tip(s, INFILL, fr, (ua, ub), iw, y1 - TIP);
                }
            }
            FenceKind::HorizontalSlats => {
                for (a, b) in pieces(y1 - y0, f.slat_width_m, f.gap_m) {
                    cuboid(s, INFILL, fr, (u0, u1), iw, (y0 + a, y0 + b), true);
                }
            }
            FenceKind::Panels | FenceKind::Screen => {
                cuboid(s, INFILL, fr, (u0, u1), iw, (y0, y1), true);
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
                (y0, y1),
            ),
        }
        if f.rails {
            for (ya, yb) in rail_heights(f, y0, y1) {
                cuboid(s, RAILS, fr, (u0, u1), rw, (ya, yb), true);
            }
        }
    }
}

/// A grid of thin wires: verticals in the front of the depth, horizontals behind them.
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
    (y0, y1): (f32, f32),
) {
    let wx = (iw.1 - wire.min(iw.1 - iw.0), iw.1);
    for (a, b) in pieces(u1 - u0, wire, gap) {
        cuboid(s, face, fr, (u0 + a, u0 + b), wx, (y0, y1), true);
    }
    let wz = (rw.0, rw.0 + wire.min(rw.1 - rw.0));
    for (a, b) in pieces(y1 - y0, wire, gap) {
        cuboid(s, face, fr, (u0, u1), wz, (y0 + a, y0 + b), true);
    }
}

/// The rail boards' bottoms and tops.
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
    let (ya, yb) = (lo + 0.08, (hi - 0.08).min(y1));
    let ya = ya.max(y0.min(lo + 0.08));
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
            (ya, yb),
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
}
