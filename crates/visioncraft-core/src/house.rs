//! What turns a block (a closed extrusion) into a house: a roof, windows and doors in its walls
//! and a plinth line, drawn by `shape_mesh` with stable face names so materials attach:
//! `<id>:roof`, `<id>:gable` (the wall above the eaves), `<id>:glass`, `<id>:frame`,
//! `<id>:door`, `<id>:interior` (the dark room behind a window), `<id>:plinth`, `<id>:parapet`.
//!
//! Kept simple rather than complete: pitched roofs on rectangles and L shapes (an L is two
//! rectangles, each with its own roof); any other outline, a curved one included, gets a flat
//! roof and a note.
//!
//! A curved wall is the tessellated outline: many short straight segments of one solid, so the
//! faces stay continuous. An opening on a curved edge is placed by arc length along the control
//! edge and is a straight unit on the chord of its arc span (see [`place`]).

use crate::curve::Tess;
use serde::{Deserialize, Serialize};

type P2 = [f32; 2];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoofKind {
    /// Flat, with a parapet of `parapet_m` round it.
    Flat,
    /// Two slopes meeting at a ridge along the longer side, gable walls at the ends.
    #[default]
    Gable,
    /// Four slopes: the gable's ends slope too.
    Hip,
    /// One slope rising across the shorter side.
    LeanTo,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Roof {
    pub kind: RoofKind,
    pub pitch_deg: f32,
    /// How far the roof reaches past the walls.
    pub overhang_m: f32,
    /// Height of a flat roof's parapet.
    pub parapet_m: f32,
}

impl Default for Roof {
    fn default() -> Self {
        Self {
            kind: RoofKind::Gable,
            pitch_deg: 35.0,
            overhang_m: 0.4,
            parapet_m: 0.4,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpeningKind {
    #[default]
    Window,
    Door,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameStyle {
    /// 5 cm frame.
    #[default]
    Slim,
    /// 10 cm frame.
    Wide,
    /// Two panes: a slim frame and a mullion down the middle.
    Split,
    /// Glass to the reveal.
    None,
}

impl FrameStyle {
    fn width(self) -> f32 {
        match self {
            Self::Slim | Self::Split => 0.05,
            Self::Wide => 0.1,
            Self::None => 0.0,
        }
    }
}

/// A window or door in one wall of the block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Opening {
    pub id: String,
    /// The wall: the outline edge `side` (as in `<id>:side(n)`).
    pub side: usize,
    /// From the edge's first corner to the opening's centre, along the wall.
    pub offset_m: f32,
    pub width_m: f32,
    pub height_m: f32,
    /// Height of its bottom above the block's base (0 for a door).
    pub sill_m: f32,
    #[serde(default)]
    pub kind: OpeningKind,
    #[serde(default)]
    pub frame: FrameStyle,
}

/// A block's house parts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct House {
    pub roof: Roof,
    pub openings: Vec<Opening>,
    /// Height of the plinth band at the foot of the walls (0: none).
    pub plinth_m: f32,
    /// Share of the windows with a lit room behind them when the lamps are on (0..1), chosen
    /// by each window's id (`lit_windows`).
    pub lit_rooms: f32,
}

/// Faces a house adds after the block's own, in this order.
pub const HOUSE_FACES: [&str; 10] = [
    "roof", "gable", "glass", "frame", "door", "interior", "plinth", "parapet", "sill", "handle",
];
const ROOF: usize = 0;
const GABLE: usize = 1;
const GLASS: usize = 2;
const FRAME: usize = 3;
const DOOR: usize = 4;
const INTERIOR: usize = 5;
const PLINTH: usize = 6;
const PARAPET: usize = 7;
const SILL: usize = 8;
const HANDLE: usize = 9;

/// How deep windows sit in the wall, and how deep the dark room behind them is.
const REVEAL_M: f32 = 0.12;
const ROOM_M: f32 = 0.6;
/// The wall's thickness for an opening on a curve: its flat unit sits in the reveal, so the
/// chord of the opening's arc span may stand at most this far (`WALL_M - REVEAL_M`) off the wall
/// surface, the sagitta of the span.
const WALL_M: f32 = 0.3;
pub const MAX_SPAN_SAGITTA_M: f32 = WALL_M - REVEAL_M;
/// Width of the parapet round a flat roof.
const PARAPET_T: f32 = 0.2;

/// Where `shape_mesh` draws: a triangle on a face (index into the block's face list) with the
/// side it faces.
pub(crate) trait Sink {
    fn tri(&mut self, face: usize, v: [[f32; 3]; 3], outward: [f32; 3]);
}

fn quad(s: &mut dyn Sink, face: usize, v: [[f32; 3]; 4], outward: [f32; 3]) {
    s.tri(face, [v[0], v[1], v[2]], outward);
    s.tri(face, [v[0], v[2], v[3]], outward);
}

/// Both sides of a roof plane (its underside is the soffit seen from below).
fn plane(s: &mut dyn Sink, face: usize, v: &[[f32; 3]], up: [f32; 3]) {
    let down = up.map(|c| -c);
    for k in 1..v.len() - 1 {
        s.tri(face, [v[0], v[k], v[k + 1]], up);
        s.tri(face, [v[0], v[k], v[k + 1]], down);
    }
}

/// Where an opening is: its extent along its control edge (arc length), its height range and
/// the flat unit that sits in it.
pub(crate) struct Placed<'a> {
    pub o: &'a Opening,
    /// From the control edge's first corner, along the edge (the arc on a curve).
    pub s0: f32,
    pub s1: f32,
    pub v0: f32,
    pub v1: f32,
    unit: Unit,
}

/// The straight unit of an opening, in the frame of its chord.
struct Unit {
    /// The chord's first end (the edge's own first corner on a straight edge).
    origin: P2,
    /// Along the chord, and its outward normal.
    d: P2,
    n: P2,
    /// The unit's extent along the chord from `origin`.
    u0: f32,
    u1: f32,
    /// How far behind the chord the deepest point of the wall surface lies in the span: the unit
    /// sits that much deeper.
    deep: f32,
    /// How far the outermost wall surface lies in front of the chord in the span.
    bulge: f32,
    /// The wall surface in plan from the span's start to its end (two points on a straight edge).
    surf: Vec<P2>,
}

impl Unit {
    /// Point at `u` along the chord, height `y`, `depth` metres into the wall from the unit's
    /// base plane.
    fn at(&self, u: f32, y: f32, depth: f32) -> [f32; 3] {
        let k = self.deep + depth;
        [
            self.origin[0] + self.d[0] * u - self.n[0] * k,
            y,
            self.origin[1] + self.d[1] * u - self.n[1] * k,
        ]
    }
}

fn dist(a: P2, b: P2) -> f32 {
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
}

/// The point at arc length `s` along polyline `run` (cumulative lengths `cum`) and the index of
/// the segment it lies on.
fn along(run: &[P2], cum: &[f32], s: f32) -> (P2, usize) {
    let k = cum
        .partition_point(|&c| c <= s)
        .saturating_sub(1)
        .min(run.len() - 2);
    let len = (cum[k + 1] - cum[k]).max(1e-9);
    let t = ((s - cum[k]) / len).clamp(0.0, 1.0);
    (
        [
            run[k][0] + (run[k + 1][0] - run[k][0]) * t,
            run[k][1] + (run[k + 1][1] - run[k][1]) * t,
        ],
        k,
    )
}

/// The openings of a house on the tessellated outline `tess` (winding `sign`), each placed by
/// arc length along its control edge, with notes for those cut down or dropped. The offset and
/// width of an opening are lengths along the edge; on a curved edge the unit is a straight piece
/// on the chord of its arc span, and a span whose chord would leave the wall (its sagitta over
/// [`MAX_SPAN_SAGITTA_M`]) is narrowed about its centre until it fits, with a note.
pub(crate) fn place<'a>(
    house: &'a House,
    tess: &Tess,
    sign: f32,
    (lo, hi): (f32, f32),
) -> (Vec<Placed<'a>>, Vec<String>) {
    let runs = tess.runs();
    let mut notes = Vec::new();
    let mut out = Vec::new();
    for o in &house.openings {
        let Some(run) = runs.get(o.side).filter(|r| r.len() >= 2) else {
            continue;
        };
        let mut cum = vec![0.0f32];
        for w in run.windows(2) {
            cum.push(cum.last().copied().unwrap_or(0.0) + dist(w[0], w[1]));
        }
        let len = cum[cum.len() - 1];
        let (mut s0, mut s1) = (
            (o.offset_m - o.width_m / 2.0).max(0.05),
            (o.offset_m + o.width_m / 2.0).min(len - 0.05),
        );
        let v0 = (lo + o.sill_m.max(0.0)).max(lo);
        let v1 = (v0 + o.height_m).min(hi - 0.05);
        if !(s1 - s0 > 0.05 && v1 - v0 > 0.05) {
            notes.push(format!(
                "opening `{}` does not fit on wall {} and is left out",
                o.id, o.side
            ));
            continue;
        }
        let unit = if run.len() == 2 {
            let l = dist(run[0], run[1]).max(1e-9);
            let d = [(run[1][0] - run[0][0]) / l, (run[1][1] - run[0][1]) / l];
            let (a, _) = along(run, &cum, s0);
            let (b, _) = along(run, &cum, s1);
            Some(Unit {
                origin: run[0],
                d,
                n: [sign * d[1], -sign * d[0]],
                u0: s0,
                u1: s1,
                deep: 0.0,
                bulge: 0.0,
                surf: vec![a, b],
            })
        } else {
            let centre = (s0 + s1) / 2.0;
            let mut half = (s1 - s0) / 2.0;
            let (full, mut unit) = (half, None);
            while half >= 0.15 {
                if let Some((u, sag)) = curved_unit(run, &cum, sign, centre - half, centre + half)
                    && sag <= MAX_SPAN_SAGITTA_M
                {
                    unit = Some(u);
                    break;
                }
                half *= 0.9;
            }
            if unit.is_some() {
                (s0, s1) = (centre - half, centre + half);
                if half < full - 1e-4 {
                    notes.push(format!(
                        "opening `{}` narrowed from {:.2} to {:.2} m: its arc span is too curved for a flat unit in a {:.2} m wall",
                        o.id,
                        2.0 * full,
                        2.0 * half,
                        WALL_M
                    ));
                }
            } else {
                notes.push(format!(
                    "opening `{}` is too curved for the wall and is left out",
                    o.id
                ));
            }
            unit
        };
        if let Some(unit) = unit {
            out.push(Placed {
                o,
                s0,
                s1,
                v0,
                v1,
                unit,
            });
        }
    }
    (out, notes)
}

/// The flat unit for the arc span `s0..s1` of a curved run and the sagitta of that span (the
/// range of the surface in front of and behind the chord).
fn curved_unit(run: &[P2], cum: &[f32], sign: f32, s0: f32, s1: f32) -> Option<(Unit, f32)> {
    let (a, ka) = along(run, cum, s0);
    let (b, kb) = along(run, cum, s1);
    let c = dist(a, b);
    if c < 0.05 {
        return None;
    }
    let d = [(b[0] - a[0]) / c, (b[1] - a[1]) / c];
    let n = [sign * d[1], -sign * d[0]];
    let mut surf = vec![a];
    surf.extend_from_slice(&run[ka + 1..=kb]);
    surf.push(b);
    surf.dedup_by(|p, q| dist(*p, *q) < 1e-5);
    let dev = |p: &P2| (p[0] - a[0]) * n[0] + (p[1] - a[1]) * n[1];
    let (bulge, back) = surf.iter().fold((0.0f32, 0.0f32), |(hi, lo), p| {
        (hi.max(dev(p)), lo.min(dev(p)))
    });
    Some((
        Unit {
            origin: a,
            d,
            n,
            u0: 0.0,
            u1: c,
            deep: -back,
            bulge,
            surf,
        },
        bulge - back,
    ))
}

/// The walls of a block with its openings and plinth, on the tessellated outline `tess` (a
/// curved edge is many short walls of one face). `sign` is its winding (the outward side of
/// edge `d` is `sign * (dz, -dx)`), `side_face(e)` the face index of control edge `e`, `first`
/// the index of the first house face. Returns notes about openings cut down or left out.
pub(crate) fn walls(
    s: &mut dyn Sink,
    house: &House,
    tess: &Tess,
    sign: f32,
    (lo, hi): (f32, f32),
    side_face: &dyn Fn(usize) -> usize,
    first: usize,
) -> Vec<String> {
    let ring = &tess.pts;
    let plinth_top = (lo + house.plinth_m.max(0.0)).min(hi);
    let (placed, notes) = place(house, tess, sign, (lo, hi));
    // Arc length at the start of each segment along its control edge.
    let mut start = vec![0.0f32; tess.edge_of.iter().max().map_or(0, |m| m + 1)];
    for i in 0..ring.len() {
        let (p, q) = (ring[i], ring[(i + 1) % ring.len()]);
        let len = dist(p, q);
        let edge = tess.edge_of[i];
        let a0 = start[edge];
        start[edge] += len;
        if len < 1e-4 {
            continue;
        }
        let d = [(q[0] - p[0]) / len, (q[1] - p[1]) / len];
        let n = [sign * d[1], -sign * d[0]];
        // Point at `u` along the wall, height `y`, `depth` metres into it.
        let at = |u: f32, y: f32, depth: f32| {
            [
                p[0] + d[0] * u - n[0] * depth,
                y,
                p[1] + d[1] * u - n[1] * depth,
            ]
        };
        let out = [n[0], 0.0, n[1]];
        // The holes in this segment: each opening's arc span clipped to it, in its own length.
        let rects: Vec<[f32; 4]> = placed
            .iter()
            .filter(|pl| pl.o.side == edge)
            .filter_map(|pl| {
                let (u0, u1) = ((pl.s0 - a0).max(0.0), (pl.s1 - a0).min(len));
                (u1 - u0 > 1e-4).then_some([u0, u1, pl.v0, pl.v1])
            })
            .collect();
        // The wall as a grid cut at every opening edge and the plinth line, less the openings.
        let mut us = vec![0.0, len];
        let mut vs = vec![lo, plinth_top, hi];
        for r in &rects {
            us.extend([r[0], r[1]]);
            vs.extend([r[2], r[3]]);
        }
        for v in [&mut us, &mut vs] {
            v.sort_by(f32::total_cmp);
            v.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
        }
        for uw in us.windows(2) {
            for vw in vs.windows(2) {
                let (cu, cv) = ((uw[0] + uw[1]) / 2.0, (vw[0] + vw[1]) / 2.0);
                if rects
                    .iter()
                    .any(|r| cu > r[0] && cu < r[1] && cv > r[2] && cv < r[3])
                {
                    continue;
                }
                let face = if vw[1] <= plinth_top + 1e-4 && plinth_top > lo {
                    first + PLINTH
                } else {
                    side_face(edge)
                };
                quad(
                    s,
                    face,
                    [
                        at(uw[0], vw[0], 0.0),
                        at(uw[1], vw[0], 0.0),
                        at(uw[1], vw[1], 0.0),
                        at(uw[0], vw[1], 0.0),
                    ],
                    out,
                );
            }
        }
    }
    for pl in &placed {
        opening(s, pl, side_face(pl.o.side), first);
    }
    notes
}

/// Fills the horizontal polygon `pts` (all at one height), facing `outward`.
fn fill(s: &mut dyn Sink, face: usize, pts: &[[f32; 3]], outward: [f32; 3]) {
    let flat: Vec<f64> = pts
        .iter()
        .flat_map(|p| [f64::from(p[0]), f64::from(p[2])])
        .collect();
    for t in earcutr::earcut(&flat, &[], 2)
        .unwrap_or_default()
        .as_chunks::<3>()
        .0
    {
        s.tri(face, [pts[t[0]], pts[t[1]], pts[t[2]]], outward);
    }
}

/// The reveal, frame, glass (or door) and the dark room behind a window.
fn opening(s: &mut dyn Sink, pl: &Placed, wall: usize, first: usize) {
    let (o, unit) = (pl.o, &pl.unit);
    let (u0, u1, v0, v1) = (unit.u0, unit.u1, pl.v0, pl.v1);
    let at = |u: f32, y: f32, depth: f32| unit.at(u, y, depth);
    let (d, n) = (unit.d, unit.n);
    let out = [n[0], 0.0, n[1]];
    let along = [d[0], 0.0, d[1]];
    let back = [-along[0], 0.0, -along[2]];
    let (up, down) = ([0.0, 1.0, 0.0], [0.0, -1.0, 0.0]);
    // Reveals: the wall's material, facing into the hole. Top and bottom follow the wall's
    // surface (the arc) out to the unit's back plane; the sides are planes through the span's
    // ends square to the chord.
    let z0 = -unit.deep;
    for (v, outward) in [(v0, up), (v1, down)] {
        if unit.surf.len() == 2 {
            quad(
                s,
                wall,
                [
                    at(u0, v, z0),
                    at(u1, v, z0),
                    at(u1, v, REVEAL_M),
                    at(u0, v, REVEAL_M),
                ],
                outward,
            );
        } else {
            let mut poly: Vec<[f32; 3]> = unit.surf.iter().map(|p| [p[0], v, p[1]]).collect();
            poly.extend([at(u1, v, REVEAL_M), at(u0, v, REVEAL_M)]);
            fill(s, wall, &poly, outward);
        }
    }
    quad(
        s,
        wall,
        [
            at(u0, v0, z0),
            at(u0, v1, z0),
            at(u0, v1, REVEAL_M),
            at(u0, v0, REVEAL_M),
        ],
        along,
    );
    quad(
        s,
        wall,
        [
            at(u1, v0, z0),
            at(u1, v1, z0),
            at(u1, v1, REVEAL_M),
            at(u1, v0, REVEAL_M),
        ],
        back,
    );
    // Frame: a ring in the reveal's back plane standing 6 cm proud of the glass, with its
    // inner sides, and a mullion for split windows.
    let f = o.frame.width().min((u1 - u0) / 4.0).min((v1 - v0) / 4.0);
    let z = REVEAL_M;
    let ring = |a: f32, b: f32, c: f32, e: f32, z: f32| {
        [at(a, c, z), at(b, c, z), at(b, e, z), at(a, e, z)]
    };
    let proud = 0.06;
    let fz = z - proud;
    let (a, b, c, e) = (u0 + f, u1 - f, v0 + f, v1 - f);
    if f > 0.0 {
        for r in [
            ring(u0, u1, v0, v0 + f, fz),
            ring(u0, u1, v1 - f, v1, fz),
            ring(u0, u0 + f, v0 + f, v1 - f, fz),
            ring(u1 - f, u1, v0 + f, v1 - f, fz),
        ] {
            quad(s, first + FRAME, r, out);
        }
        // The frame's inner sides, from its face back to the glass.
        quad(
            s,
            first + FRAME,
            [at(a, c, fz), at(b, c, fz), at(b, c, z), at(a, c, z)],
            up,
        );
        quad(
            s,
            first + FRAME,
            [at(a, e, fz), at(b, e, fz), at(b, e, z), at(a, e, z)],
            down,
        );
        quad(
            s,
            first + FRAME,
            [at(a, c, fz), at(a, e, fz), at(a, e, z), at(a, c, z)],
            along,
        );
        quad(
            s,
            first + FRAME,
            [at(b, c, fz), at(b, e, fz), at(b, e, z), at(b, c, z)],
            back,
        );
        if o.frame == FrameStyle::Split && o.kind == OpeningKind::Window {
            let m = (u0 + u1) / 2.0;
            quad(
                s,
                first + FRAME,
                ring(m - f / 2.0, m + f / 2.0, c, e, fz),
                out,
            );
        }
    }
    match o.kind {
        OpeningKind::Door => {
            // A panelled door: its leaf flush with the frame, four panels set 1.5 cm back in it,
            // and a handle at the right height on the side away from the hinges.
            let (w, h) = (b - a, e - c);
            let (bx, by) = ((w * 0.12).max(0.06), (h * 0.08).max(0.06));
            let mid_u = (a + b) / 2.0;
            let mid_v = c + h * 0.45;
            let panels = [
                (a + bx, mid_u - bx / 2.0, c + by, mid_v - by / 2.0),
                (mid_u + bx / 2.0, b - bx, c + by, mid_v - by / 2.0),
                (a + bx, mid_u - bx / 2.0, mid_v + by / 2.0, e - by),
                (mid_u + bx / 2.0, b - bx, mid_v + by / 2.0, e - by),
            ];
            // The leaf around the panels, on a grid less the panels.
            let mut us = vec![a, b];
            let mut vs = vec![c, e];
            for p in &panels {
                us.extend([p.0, p.1]);
                vs.extend([p.2, p.3]);
            }
            for v in [&mut us, &mut vs] {
                v.sort_by(f32::total_cmp);
                v.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
            }
            let inset = 0.025;
            // The leaf sits 2 cm behind the frame's face, so the frame reads around it.
            let fz = fz + 0.02;
            for uw in us.windows(2) {
                for vw in vs.windows(2) {
                    let (cu, cv) = ((uw[0] + uw[1]) / 2.0, (vw[0] + vw[1]) / 2.0);
                    let inside = panels
                        .iter()
                        .any(|p| cu > p.0 && cu < p.1 && cv > p.2 && cv < p.3);
                    let depth = if inside { fz + inset } else { fz };
                    quad(
                        s,
                        first + DOOR,
                        ring(uw[0], uw[1], vw[0], vw[1], depth),
                        out,
                    );
                }
            }
            // The panels' sides.
            for p in &panels {
                let (p0, p1, q0, q1) = *p;
                let d = fz + inset;
                quad(
                    s,
                    first + DOOR,
                    [at(p0, q0, fz), at(p1, q0, fz), at(p1, q0, d), at(p0, q0, d)],
                    up,
                );
                quad(
                    s,
                    first + DOOR,
                    [at(p0, q1, fz), at(p1, q1, fz), at(p1, q1, d), at(p0, q1, d)],
                    down,
                );
                quad(
                    s,
                    first + DOOR,
                    [at(p0, q0, fz), at(p0, q1, fz), at(p0, q1, d), at(p0, q0, d)],
                    along,
                );
                quad(
                    s,
                    first + DOOR,
                    [at(p1, q0, fz), at(p1, q1, fz), at(p1, q1, d), at(p1, q0, d)],
                    back,
                );
            }
            // The handle: a small block 6 cm out from the leaf, 1.05 m up.
            let hu = b - (w * 0.1).max(0.07);
            let hv = (c + 1.05).min(e - 0.1);
            block(
                s,
                first + HANDLE,
                &at,
                (hu - 0.06, hu + 0.03),
                (hv - 0.015, hv + 0.015),
                (fz - 0.06, fz),
                (out, along, up),
            );
        }
        OpeningKind::Window => {
            // The pane a little behind the frame, then the room behind it: its back wall,
            // floor, ceiling and sides, dark unless a lamp inside lights them.
            let g = z - 0.01;
            quad(
                s,
                first + GLASS,
                [at(a, c, g), at(b, c, g), at(b, e, g), at(a, e, g)],
                out,
            );
            let r = z + ROOM_M;
            let room = first + INTERIOR;
            quad(
                s,
                room,
                [at(u0, v0, r), at(u1, v0, r), at(u1, v1, r), at(u0, v1, r)],
                out,
            );
            quad(
                s,
                room,
                [at(u0, v0, z), at(u1, v0, z), at(u1, v0, r), at(u0, v0, r)],
                up,
            );
            quad(
                s,
                room,
                [at(u0, v1, z), at(u1, v1, z), at(u1, v1, r), at(u0, v1, r)],
                down,
            );
            quad(
                s,
                room,
                [at(u0, v0, z), at(u0, v1, z), at(u0, v1, r), at(u0, v0, r)],
                along,
            );
            quad(
                s,
                room,
                [at(u1, v0, z), at(u1, v1, z), at(u1, v1, r), at(u1, v0, r)],
                back,
            );
            // A sill under it: 5 cm thick, reaching 6 cm past the wall and 5 cm past each side.
            if o.sill_m > 0.05 {
                block(
                    s,
                    first + SILL,
                    &at,
                    (u0 - 0.05, u1 + 0.05),
                    (v0 - 0.05, v0),
                    (-0.06 - unit.deep - unit.bulge, REVEAL_M),
                    (out, along, up),
                );
            }
        }
    }
}

/// A box on the wall: `u` along it, `v` up, `z` into it (negative: out in front), its faces
/// facing `out` (front), `along` (right), up and their opposites.
fn block(
    s: &mut dyn Sink,
    face: usize,
    at: &dyn Fn(f32, f32, f32) -> [f32; 3],
    (u0, u1): (f32, f32),
    (v0, v1): (f32, f32),
    (z0, z1): (f32, f32),
    (out, along, up): ([f32; 3], [f32; 3], [f32; 3]),
) {
    let neg = |v: [f32; 3]| v.map(|c| -c);
    quad(
        s,
        face,
        [
            at(u0, v0, z0),
            at(u1, v0, z0),
            at(u1, v1, z0),
            at(u0, v1, z0),
        ],
        out,
    );
    quad(
        s,
        face,
        [
            at(u0, v1, z0),
            at(u1, v1, z0),
            at(u1, v1, z1),
            at(u0, v1, z1),
        ],
        up,
    );
    quad(
        s,
        face,
        [
            at(u0, v0, z0),
            at(u1, v0, z0),
            at(u1, v0, z1),
            at(u0, v0, z1),
        ],
        neg(up),
    );
    quad(
        s,
        face,
        [
            at(u0, v0, z0),
            at(u0, v1, z0),
            at(u0, v1, z1),
            at(u0, v0, z1),
        ],
        neg(along),
    );
    quad(
        s,
        face,
        [
            at(u1, v0, z0),
            at(u1, v1, z0),
            at(u1, v1, z1),
            at(u1, v0, z1),
        ],
        along,
    );
}

/// Where the lit rooms' lamps hang: for each window of block `e` chosen by `House::lit_rooms`
/// (by a hash of its id, so the same windows stay lit), a point in the room behind it, above the
/// middle, in the block's own coordinates (add the block's lift).
pub fn lit_windows(e: &crate::shape::Extrusion) -> Vec<[f32; 3]> {
    let Some(house) = e.house.as_ref().filter(|h| h.lit_rooms > 0.0) else {
        return Vec::new();
    };
    if e.points.len() < 3 {
        return Vec::new();
    }
    let tess = crate::curve::tessellate(&e.points, true, &e.curves, e.smooth);
    let sign = if crate::shape::shoelace_f32(&tess.pts) > 0.0 {
        1.0
    } else {
        -1.0
    };
    place(house, &tess, sign, (e.base, e.base + e.height))
        .0
        .iter()
        .filter(|pl| pl.o.kind == OpeningKind::Window)
        .filter(|pl| {
            let h = pl.o.id.bytes().fold(2166136261u32, |h, b| {
                (h ^ u32::from(b)).wrapping_mul(16777619)
            });
            (h % 1000) as f32 / 1000.0 < house.lit_rooms
        })
        .map(|pl| {
            let u = &pl.unit;
            u.at(
                (u.u0 + u.u1) / 2.0,
                e.base + pl.o.sill_m + pl.o.height_m * 0.75,
                REVEAL_M + ROOM_M * 0.5,
            )
        })
        .collect()
}

/// A rectangle as its four corners (as given) when `ring` is one: four corners at right angles.
fn rectangle(ring: &[P2]) -> Option<[P2; 4]> {
    let r: [P2; 4] = ring.try_into().ok()?;
    let e = |i: usize| [r[(i + 1) % 4][0] - r[i][0], r[(i + 1) % 4][1] - r[i][1]];
    let square = |a: P2, b: P2| {
        let l = (a[0].hypot(a[1]) * b[0].hypot(b[1])).max(1e-9);
        ((a[0] * b[0] + a[1] * b[1]) / l).abs() < 1e-3
    };
    (0..4).all(|i| square(e(i), e((i + 1) % 4))).then_some(r)
}

/// An L-shaped outline (six corners at right angles) as two rectangles, or `None`.
pub(crate) fn split_l(ring: &[P2]) -> Option<[[P2; 4]; 2]> {
    if ring.len() != 6 {
        return None;
    }
    // Into the frame of the first edge, where every edge is along x or z.
    let (dx, dz) = (ring[1][0] - ring[0][0], ring[1][1] - ring[0][1]);
    let l = dx.hypot(dz);
    if l < 1e-6 {
        return None;
    }
    let (c, s) = (dx / l, dz / l);
    let to = |p: P2| [p[0] * c + p[1] * s, -p[0] * s + p[1] * c];
    let from = |p: P2| [p[0] * c - p[1] * s, p[0] * s + p[1] * c];
    let local: Vec<P2> = ring.iter().map(|&p| to(p)).collect();
    for i in 0..6 {
        let (a, b) = (local[i], local[(i + 1) % 6]);
        if (a[0] - b[0]).abs() > 1e-3 && (a[1] - b[1]).abs() > 1e-3 {
            return None; // not axis-aligned: no L
        }
    }
    let (x0, x1) = local
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
    let (z0, z1) = local
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    let on = |v: f32, a: f32, b: f32| (v - a).abs() < 1e-3 || (v - b).abs() < 1e-3;
    // The reflex corner is the one not on the bounding box's border in either coordinate.
    let r = *local
        .iter()
        .find(|p| !on(p[0], x0, x1) && !on(p[1], z0, z1))?;
    // The notch lies towards the box corner the reflex corner's neighbours point away from:
    // take the box corner nearest to r that is not in the outline.
    let corners = [[x0, z0], [x1, z0], [x1, z1], [x0, z1]];
    let notch = *corners.iter().find(|k| {
        !local
            .iter()
            .any(|p| (p[0] - k[0]).abs() < 1e-3 && (p[1] - k[1]).abs() < 1e-3)
    })?;
    // The full-width strip on the side of r away from the notch (along z), and the rest.
    let (strip_z0, strip_z1, rest_z0, rest_z1) = if notch[1] > r[1] {
        (z0, r[1], r[1], z1)
    } else {
        (r[1], z1, z0, r[1])
    };
    let (rest_x0, rest_x1) = if notch[0] > r[0] {
        (x0, r[0])
    } else {
        (r[0], x1)
    };
    let rect = |a: f32, b: f32, c: f32, e: f32| [[a, c], [b, c], [b, e], [a, e]].map(from);
    Some([
        rect(x0, x1, strip_z0, strip_z1),
        rect(rest_x0, rest_x1, rest_z0, rest_z1),
    ])
}

/// The roof of a block; a note when the outline gets a flat roof instead of the one asked for.
pub(crate) fn roof(
    s: &mut dyn Sink,
    house: &House,
    ring: &[P2],
    hi: f32,
    first: usize,
) -> Option<String> {
    let r = &house.roof;
    let pitched = r.kind != RoofKind::Flat;
    let parts: Option<Vec<[P2; 4]>> = rectangle(ring)
        .map(|q| vec![q])
        .or_else(|| split_l(ring).map(|l| l.to_vec()));
    match (pitched, parts) {
        (true, Some(parts)) => {
            for q in parts {
                pitched_roof(s, r, q, hi, first);
            }
            None
        }
        (true, None) => {
            flat_roof(s, r, ring, hi, first);
            Some("a pitched roof needs a rectangular or L-shaped outline with straight walls: drawn flat (use a flat roof on a curved outline)".into())
        }
        (false, _) => {
            flat_roof(s, r, ring, hi, first);
            None
        }
    }
}

fn flat_roof(s: &mut dyn Sink, r: &Roof, ring: &[P2], hi: f32, first: usize) {
    let up = [0.0, 1.0, 0.0];
    // The roof deck just above the top; earcut through the shape's own cap.
    let flat: Vec<f64> = ring
        .iter()
        .flat_map(|p| [f64::from(p[0]), f64::from(p[1])])
        .collect();
    for t in earcutr::earcut(&flat, &[], 2)
        .unwrap_or_default()
        .as_chunks::<3>()
        .0
    {
        let v = [t[0], t[1], t[2]].map(|i| [ring[i][0], hi + 0.01, ring[i][1]]);
        s.tri(first + ROOF, v, up);
    }
    // The parapet: a 20 cm wall round the edge, outside and inside faces and a top.
    let h = r.parapet_m.max(0.0);
    if h <= 0.0 {
        return;
    }
    let sign = if crate::shape::shoelace_f32(ring) > 0.0 {
        1.0
    } else {
        -1.0
    };
    // The inner face is the outline offset by the parapet's width, mitred at every vertex, so
    // the faces meet along a curve of many short segments as along a straight wall.
    let m = ring.len();
    let mut normals: Vec<P2> = Vec::with_capacity(m);
    for i in 0..m {
        let (p, q) = (ring[i], ring[(i + 1) % m]);
        let len = dist(p, q);
        normals.push(if len < 1e-4 {
            normals.last().copied().unwrap_or([0.0, 0.0])
        } else {
            [sign * (q[1] - p[1]) / len, -sign * (q[0] - p[0]) / len]
        });
    }
    if normals[0] == [0.0, 0.0] {
        let fix = normals.iter().copied().find(|n| *n != [0.0, 0.0]);
        if let Some(f) = fix {
            normals[0] = f;
        }
    }
    let inner: Vec<P2> = (0..m)
        .map(|i| {
            let (a, b) = (normals[(i + m - 1) % m], normals[i]);
            let k = PARAPET_T / (1.0 + a[0] * b[0] + a[1] * b[1]).max(0.25);
            [
                ring[i][0] - (a[0] + b[0]) * k,
                ring[i][1] - (a[1] + b[1]) * k,
            ]
        })
        .collect();
    let c = |a: P2, y: f32| [a[0], y, a[1]];
    for i in 0..m {
        let j = (i + 1) % m;
        let (p, q) = (ring[i], ring[j]);
        if dist(p, q) < 1e-4 {
            continue;
        }
        let n = normals[i];
        let (pi, qi) = (inner[i], inner[j]);
        let face = first + PARAPET;
        quad(
            s,
            face,
            [c(p, hi), c(q, hi), c(q, hi + h), c(p, hi + h)],
            [n[0], 0.0, n[1]],
        );
        quad(
            s,
            face,
            [c(pi, hi), c(qi, hi), c(qi, hi + h), c(pi, hi + h)],
            [-n[0], 0.0, -n[1]],
        );
        quad(
            s,
            face,
            [c(p, hi + h), c(q, hi + h), c(qi, hi + h), c(pi, hi + h)],
            [0.0, 1.0, 0.0],
        );
    }
}

/// A gable, hip or lean-to roof over rectangle `q`, its ridge along the longer side.
fn pitched_roof(s: &mut dyn Sink, r: &Roof, q: [P2; 4], hi: f32, first: usize) {
    let len = |a: P2, b: P2| ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
    // Corners a b c d with a-b along the ridge (the longer side).
    let [a, b, c, d] = if len(q[0], q[1]) >= len(q[1], q[2]) {
        q
    } else {
        [q[1], q[2], q[3], q[0]]
    };
    let (l, w) = (len(a, b), len(b, c));
    if l < 1e-3 || w < 1e-3 {
        return;
    }
    let u = [(b[0] - a[0]) / l, (b[1] - a[1]) / l]; // along the ridge
    let v = [(d[0] - a[0]) / w, (d[1] - a[1]) / w]; // across, from a-b to d-c
    let tan = r.pitch_deg.clamp(5.0, 70.0).to_radians().tan();
    let o = r.overhang_m.clamp(0.0, 2.0);
    // A point `x` along and `y` across from a, at height `h`.
    let pt = |x: f32, y: f32, h: f32| [a[0] + u[0] * x + v[0] * y, h, a[1] + u[1] * x + v[1] * y];
    let up = |nx: f32, ny: f32| {
        // The upward normal of a plane falling towards (nx along, ny across).
        let k = (nx * nx + ny * ny).sqrt().max(1e-6);
        let (gx, gy) = (nx / k, ny / k);
        let horiz = [u[0] * gx + v[0] * gy, u[1] * gx + v[1] * gy];
        [horiz[0] * tan, 1.0, horiz[1] * tan]
    };
    let gable = first + GABLE;
    let roof = first + ROOF;
    // Eaves at wall-top height on the wall line, lower by the overhang's fall past it.
    let eave = hi - o * tan;
    match r.kind {
        RoofKind::LeanTo => {
            let top = hi + w * tan;
            plane(
                s,
                roof,
                &[
                    pt(-o, -o, eave),
                    pt(l + o, -o, eave),
                    pt(l + o, w + o, top + o * tan),
                    pt(-o, w + o, top + o * tan),
                ],
                up(0.0, -1.0),
            );
            // The high wall and the two triangles at the ends.
            quad(
                s,
                gable,
                [pt(0.0, w, hi), pt(l, w, hi), pt(l, w, top), pt(0.0, w, top)],
                [v[0], 0.0, v[1]],
            );
            s.tri(
                gable,
                [pt(0.0, 0.0, hi), pt(0.0, w, hi), pt(0.0, w, top)],
                [-u[0], 0.0, -u[1]],
            );
            s.tri(
                gable,
                [pt(l, 0.0, hi), pt(l, w, hi), pt(l, w, top)],
                [u[0], 0.0, u[1]],
            );
        }
        RoofKind::Gable | RoofKind::Flat => {
            let ridge = hi + w / 2.0 * tan;
            let m = w / 2.0;
            plane(
                s,
                roof,
                &[
                    pt(-o, -o, eave),
                    pt(l + o, -o, eave),
                    pt(l + o, m, ridge),
                    pt(-o, m, ridge),
                ],
                up(0.0, -1.0),
            );
            plane(
                s,
                roof,
                &[
                    pt(-o, w + o, eave),
                    pt(l + o, w + o, eave),
                    pt(l + o, m, ridge),
                    pt(-o, m, ridge),
                ],
                up(0.0, 1.0),
            );
            s.tri(
                gable,
                [pt(0.0, 0.0, hi), pt(0.0, w, hi), pt(0.0, m, ridge)],
                [-u[0], 0.0, -u[1]],
            );
            s.tri(
                gable,
                [pt(l, 0.0, hi), pt(l, w, hi), pt(l, m, ridge)],
                [u[0], 0.0, u[1]],
            );
        }
        RoofKind::Hip => {
            let ridge = hi + w / 2.0 * tan;
            let m = w / 2.0;
            // The ridge stops half the width from each end (a point when the plan is square).
            let (r0, r1) = if l > w {
                (m, l - m)
            } else {
                (l / 2.0, l / 2.0)
            };
            let (e0, e1, f0, f1) = (-o, l + o, -o, w + o);
            plane(
                s,
                roof,
                &[
                    pt(e0, f0, eave),
                    pt(e1, f0, eave),
                    pt(r1, m, ridge),
                    pt(r0, m, ridge),
                ],
                up(0.0, -1.0),
            );
            plane(
                s,
                roof,
                &[
                    pt(e0, f1, eave),
                    pt(e1, f1, eave),
                    pt(r1, m, ridge),
                    pt(r0, m, ridge),
                ],
                up(0.0, 1.0),
            );
            plane(
                s,
                roof,
                &[pt(e0, f0, eave), pt(e0, f1, eave), pt(r0, m, ridge)],
                up(-1.0, 0.0),
            );
            plane(
                s,
                roof,
                &[pt(e1, f0, eave), pt(e1, f1, eave), pt(r1, m, ridge)],
                up(1.0, 0.0),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_l_splits_into_two_rectangles_covering_it() {
        // 10 x 8 with a 4 x 3 notch at the far corner.
        let l = [
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 5.0],
            [6.0, 5.0],
            [6.0, 8.0],
            [0.0, 8.0],
        ];
        let [a, b] = split_l(&l).unwrap();
        let area = |r: [P2; 4]| {
            let e = |i: usize, j: usize| {
                ((r[j][0] - r[i][0]).powi(2) + (r[j][1] - r[i][1]).powi(2)).sqrt()
            };
            e(0, 1) * e(1, 2)
        };
        assert!(
            (area(a) + area(b) - (80.0 - 12.0)).abs() < 1e-3,
            "{a:?} {b:?}"
        );
        assert!(rectangle(&l).is_none());
        assert!(split_l(&[[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]]).is_none());
        // Rotated by 30 degrees it still splits.
        let (c, s) = (30f32.to_radians().cos(), 30f32.to_radians().sin());
        let rot: Vec<P2> = l
            .iter()
            .map(|p| [p[0] * c - p[1] * s, p[0] * s + p[1] * c])
            .collect();
        let [a, b] = split_l(&rot).unwrap();
        assert!((area(a) + area(b) - 68.0).abs() < 1e-2);
    }
}
