//! What turns a block (a closed extrusion) into a house: a roof, windows and doors in its walls
//! and a plinth line, drawn by `shape_mesh` with stable face names so materials attach:
//! `<id>:roof`, `<id>:gable` (the wall above the eaves), `<id>:glass`, `<id>:frame`,
//! `<id>:door`, `<id>:interior` (the dark room behind a window), `<id>:plinth`, `<id>:parapet`.
//!
//! Kept simple rather than complete: pitched roofs on rectangles and L shapes (an L is two
//! rectangles, each with its own roof); any other outline gets a flat roof and a note.

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
}

/// Faces a house adds after the block's own, in this order.
pub const HOUSE_FACES: [&str; 8] = [
    "roof", "gable", "glass", "frame", "door", "interior", "plinth", "parapet",
];
const ROOF: usize = 0;
const GABLE: usize = 1;
const GLASS: usize = 2;
const FRAME: usize = 3;
const DOOR: usize = 4;
const INTERIOR: usize = 5;
const PLINTH: usize = 6;
const PARAPET: usize = 7;

/// How deep windows sit in the wall, and how deep the dark room behind them is.
const REVEAL_M: f32 = 0.12;
const ROOM_M: f32 = 0.6;

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

/// The walls of a block with its openings and plinth. `ring` is the outline, `sign` its
/// winding (the outward side of edge `d` is `sign * (dz, -dx)`), `side_face(i)` the face index
/// of wall `i`, `first` the index of the first house face.
pub(crate) fn walls(
    s: &mut dyn Sink,
    house: &House,
    ring: &[P2],
    sign: f32,
    (lo, hi): (f32, f32),
    side_face: &dyn Fn(usize) -> usize,
    first: usize,
) {
    let plinth_top = (lo + house.plinth_m.max(0.0)).min(hi);
    for i in 0..ring.len() {
        let (p, q) = (ring[i], ring[(i + 1) % ring.len()]);
        let len = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt();
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
        let rects: Vec<(&Opening, [f32; 4])> = house
            .openings
            .iter()
            .filter(|o| o.side == i)
            .filter_map(|o| {
                let u0 = (o.offset_m - o.width_m / 2.0).max(0.05);
                let u1 = (o.offset_m + o.width_m / 2.0).min(len - 0.05);
                let v0 = (lo + o.sill_m.max(0.0)).max(lo);
                let v1 = (v0 + o.height_m).min(hi - 0.05);
                (u1 - u0 > 0.05 && v1 - v0 > 0.05).then_some((o, [u0, u1, v0, v1]))
            })
            .collect();
        // The wall as a grid cut at every opening edge and the plinth line, less the openings.
        let mut us = vec![0.0, len];
        let mut vs = vec![lo, plinth_top, hi];
        for (_, r) in &rects {
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
                    .any(|(_, r)| cu > r[0] && cu < r[1] && cv > r[2] && cv < r[3])
                {
                    continue;
                }
                let face = if vw[1] <= plinth_top + 1e-4 && plinth_top > lo {
                    first + PLINTH
                } else {
                    side_face(i)
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
        for (o, [u0, u1, v0, v1]) in rects {
            opening(s, o, (u0, u1, v0, v1), &at, (d, n), side_face(i), first);
        }
    }
}

/// The reveal, frame, glass (or door) and the dark room behind a window.
fn opening(
    s: &mut dyn Sink,
    o: &Opening,
    (u0, u1, v0, v1): (f32, f32, f32, f32),
    at: &dyn Fn(f32, f32, f32) -> [f32; 3],
    (d, n): (P2, P2),
    wall: usize,
    first: usize,
) {
    let out = [n[0], 0.0, n[1]];
    let along = [d[0], 0.0, d[1]];
    let back = [-along[0], 0.0, -along[2]];
    let (up, down) = ([0.0, 1.0, 0.0], [0.0, -1.0, 0.0]);
    // Reveals: the wall's material, facing into the hole.
    quad(
        s,
        wall,
        [
            at(u0, v0, 0.0),
            at(u1, v0, 0.0),
            at(u1, v0, REVEAL_M),
            at(u0, v0, REVEAL_M),
        ],
        up,
    );
    quad(
        s,
        wall,
        [
            at(u0, v1, 0.0),
            at(u1, v1, 0.0),
            at(u1, v1, REVEAL_M),
            at(u0, v1, REVEAL_M),
        ],
        down,
    );
    quad(
        s,
        wall,
        [
            at(u0, v0, 0.0),
            at(u0, v1, 0.0),
            at(u0, v1, REVEAL_M),
            at(u0, v0, REVEAL_M),
        ],
        along,
    );
    quad(
        s,
        wall,
        [
            at(u1, v0, 0.0),
            at(u1, v1, 0.0),
            at(u1, v1, REVEAL_M),
            at(u1, v0, REVEAL_M),
        ],
        back,
    );
    // Frame: a ring in the reveal's back plane, and a mullion for split windows.
    let f = o.frame.width().min((u1 - u0) / 4.0).min((v1 - v0) / 4.0);
    let z = REVEAL_M;
    let ring =
        |a: f32, b: f32, c: f32, e: f32| [at(a, c, z), at(b, c, z), at(b, e, z), at(a, e, z)];
    if f > 0.0 {
        for r in [
            ring(u0, u1, v0, v0 + f),
            ring(u0, u1, v1 - f, v1),
            ring(u0, u0 + f, v0 + f, v1 - f),
            ring(u1 - f, u1, v0 + f, v1 - f),
        ] {
            quad(s, first + FRAME, r, out);
        }
        if o.frame == FrameStyle::Split && o.kind == OpeningKind::Window {
            let m = (u0 + u1) / 2.0;
            quad(
                s,
                first + FRAME,
                ring(m - f / 2.0, m + f / 2.0, v0 + f, v1 - f),
                out,
            );
        }
    }
    let (a, b, c, e) = (u0 + f, u1 - f, v0 + f, v1 - f);
    match o.kind {
        OpeningKind::Door => quad(s, first + DOOR, ring(a, b, c, e), out),
        OpeningKind::Window => {
            // The pane a little in front of the frame's back, then the room behind it: its back
            // wall, floor, ceiling and sides, dark unless a lamp inside lights them.
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
        }
    }
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
            Some("a pitched roof needs a rectangular or L-shaped outline: drawn flat".into())
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
    let t = 0.2;
    for i in 0..ring.len() {
        let (p, q) = (ring[i], ring[(i + 1) % ring.len()]);
        let len = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt();
        if len < 1e-4 {
            continue;
        }
        let n = [sign * (q[1] - p[1]) / len, -sign * (q[0] - p[0]) / len];
        let inn = |a: P2| [a[0] - n[0] * t, a[1] - n[1] * t];
        let c = |a: P2, y: f32| [a[0], y, a[1]];
        let (pi, qi) = (inn(p), inn(q));
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
