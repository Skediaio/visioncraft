//! Parametric furniture: what turns a closed extrusion with a rectangular (or, for a round table,
//! circular) footprint into a table, chair, sofa and so on, drawn by `shape_mesh` with stable
//! face names so materials attach. Only the faces a kind has exist:
//! `<id>:frame` (legs, frames, poles, bases, door fronts), `<id>:top` (table tops, seat shells,
//! headboards, cabinet bodies), `<id>:cushion` (cushions, mattresses, pillows) and
//! `<id>:canopy` (a parasol's cloth).
//!
//! The footprint is the piece's length and depth and the extrusion's `height` its overall
//! height. Parts scale with them sensibly: legs keep their thickness, a sofa's seat cushions are
//! split by count. A piece stands on `base` with its own bottom flat.
//!
//! Frame of a piece: the outline's first edge (point 0 to 1) is the length axis `u`, the second
//! (point 1 to 2) the depth axis `v`, and the front is the `+v` side (a chair is faced towards
//! `+v`, a sofa's back is at `-v`). A lounger and a bed lie along `u` with their head at `-u`.
//! [`rect_outline`] makes such an outline from a centre and a turn (counter-clockwise from above,
//! -Z is north, +X east); [`round_outline`] a circle for round tables.
//!
//! Every piece is built from boxes, prisms and canopy planes that stand on each other; boxes that
//! rest on another leave out the face they hide. Legs sink a few millimetres into what they
//! carry, so no two faces lie in one plane.

use crate::house::Sink;
use serde::{Deserialize, Serialize};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

type P2 = [f32; 2];
type P3 = [f32; 3];

/// The kinds of furniture, in the order the apps list them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FurnitureKind {
    Table,
    Chair,
    Bench,
    Sofa,
    Armchair,
    Lounger,
    Parasol,
    SideTable,
    Cabinet,
    Bed,
}

impl FurnitureKind {
    pub const ALL: [Self; 10] = [
        Self::Table,
        Self::Chair,
        Self::Bench,
        Self::Sofa,
        Self::Armchair,
        Self::Lounger,
        Self::Parasol,
        Self::SideTable,
        Self::Cabinet,
        Self::Bed,
    ];

    /// The snake_case name used in JSON and listed by the commands.
    pub fn name(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Chair => "chair",
            Self::Bench => "bench",
            Self::Sofa => "sofa",
            Self::Armchair => "armchair",
            Self::Lounger => "lounger",
            Self::Parasol => "parasol",
            Self::SideTable => "side_table",
            Self::Cabinet => "cabinet",
            Self::Bed => "bed",
        }
    }

    /// The kind called `name` (snake_case).
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    /// The faces the kind has, in order.
    pub fn faces(self) -> &'static [&'static str] {
        match self {
            Self::Table | Self::Chair | Self::Bench | Self::SideTable | Self::Cabinet => {
                &["top", "frame"]
            }
            Self::Sofa | Self::Armchair => &["top", "frame", "cushion"],
            Self::Lounger => &["frame", "cushion"],
            Self::Parasol => &["frame", "canopy"],
            Self::Bed => &["frame", "top", "cushion"],
        }
    }

    /// The palette material each face starts with.
    pub fn default_materials(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Table | Self::Chair | Self::Bench | Self::SideTable => {
                &[("top", "teak"), ("frame", "powder-coat-black")]
            }
            Self::Sofa | Self::Armchair => &[
                ("top", "teak"),
                ("frame", "powder-coat-black"),
                ("cushion", "fabric-sand"),
            ],
            Self::Lounger => &[("frame", "powder-coat-black"), ("cushion", "fabric-sand")],
            Self::Parasol => &[("frame", "powder-coat-black"), ("canopy", "canvas-white")],
            Self::Cabinet => &[("top", "teak"), ("frame", "teak")],
            Self::Bed => &[
                ("frame", "teak"),
                ("top", "teak"),
                ("cushion", "canvas-white"),
            ],
        }
    }

    /// Whether the kind can stand on a circular footprint.
    pub fn can_be_round(self) -> bool {
        matches!(self, Self::Table | Self::SideTable | Self::Parasol)
    }
}

/// Faces of a bench or bed that exist only some of the time are drawn regardless of kind lists;
/// these are the sizes a kind starts with: `[length, depth, height]` in metres.
fn default_size(kind: FurnitureKind, back: bool, round: bool) -> [f32; 3] {
    match kind {
        FurnitureKind::Table if round => [1.2, 1.2, 0.75],
        FurnitureKind::Table => [1.8, 0.9, 0.75],
        FurnitureKind::Chair => [0.45, 0.5, 0.85],
        FurnitureKind::Bench if back => [1.5, 0.5, 0.85],
        FurnitureKind::Bench => [1.5, 0.45, 0.45],
        FurnitureKind::Sofa => [2.2, 0.9, 0.8],
        FurnitureKind::Armchair => [0.9, 0.9, 0.8],
        FurnitureKind::Lounger => [1.95, 0.7, 0.85],
        FurnitureKind::Parasol => [3.0, 3.0, 2.4],
        FurnitureKind::SideTable => [0.45, 0.45, 0.5],
        FurnitureKind::Cabinet => [1.2, 0.45, 0.9],
        FurnitureKind::Bed => [2.1, 1.6, 0.9],
    }
}

/// The smallest and largest side of a piece (metres).
pub const SIDE_M: (f32, f32) = (0.1, 8.0);
/// The smallest and largest overall height of a piece (metres).
pub const HEIGHT_M: (f32, f32) = (0.1, 4.0);
/// The back angles a lounger takes (degrees; 0 is flat).
pub const BACK_DEG: (f32, f32) = (0.0, 80.0);
/// The back angle a lounger starts with.
pub const DEFAULT_BACK_DEG: f32 = 40.0;

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// A piece of furniture: its kind and the options that kind has. The other options of other
/// kinds are kept untouched and ignored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Furniture {
    pub kind: FurnitureKind,
    /// A bench with a back (default) or a plain seat.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub back: bool,
    /// A lounger's back rest angle above flat, 0..=80 degrees. Default [`DEFAULT_BACK_DEG`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back_deg: Option<f32>,
    /// A parasol with its canopy open (default) or folded round the pole.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub open: bool,
    /// How many seat cushions a sofa or armchair has, 1..=8; without it one for each 0.6 to
    /// 0.8 m between the arms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cushions: Option<u32>,
    /// How many doors a cabinet has side by side, 1..=8; without it one for each 0.45 m.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doors: Option<u32>,
    /// Rows of drawer fronts at the top of a cabinet, 0..=4.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub drawers: u32,
    /// A table on a central pedestal instead of four legs; a round table starts with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pedestal: Option<bool>,
}

impl Furniture {
    pub fn new(kind: FurnitureKind) -> Self {
        Self {
            kind,
            back: true,
            back_deg: None,
            open: true,
            cushions: None,
            doors: None,
            drawers: 0,
            pedestal: None,
        }
    }

    /// The `[length, depth, height]` a piece of this kind starts with.
    pub fn default_size(&self, round: bool) -> [f32; 3] {
        default_size(self.kind, self.back, round)
    }

    /// The faces this piece has, in order.
    pub fn faces(&self) -> &'static [&'static str] {
        self.kind.faces()
    }

    /// The lounger's back angle in degrees.
    pub fn back_angle(&self) -> f32 {
        self.back_deg.unwrap_or(DEFAULT_BACK_DEG)
    }

    /// Checks the options against their ranges.
    pub fn check(&self) -> Result<(), String> {
        if let Some(a) = self.back_deg
            && !(a.is_finite() && (BACK_DEG.0..=BACK_DEG.1).contains(&a))
        {
            return Err("`back_deg` must be within 0..=80 degrees".into());
        }
        if let Some(n) = self.cushions
            && !(1..=8).contains(&n)
        {
            return Err("`cushions` must be 1..=8".into());
        }
        if let Some(n) = self.doors
            && !(1..=8).contains(&n)
        {
            return Err("`doors` must be 1..=8".into());
        }
        if self.drawers > 4 {
            return Err("`drawers` must be 0..=4 rows".into());
        }
        Ok(())
    }

    /// How many seat cushions a sofa or armchair of `length` has (and as many back cushions).
    pub fn cushion_count(&self, length: f32) -> usize {
        let inner = (length - 2.0 * arm_width(length)).max(0.1);
        match self.cushions {
            Some(n) => n as usize,
            None => ((inner / 0.7).round() as usize).clamp(1, 8),
        }
    }

    /// How many cabinet doors stand side by side for `length`.
    pub fn door_count(&self, length: f32) -> usize {
        match self.doors {
            Some(n) => n as usize,
            None => ((length / 0.45).round() as usize).clamp(1, 8),
        }
    }

    /// The piece's frame on a 4-point `outline` (`curved`: it has arcs, a circle) standing
    /// `height` tall; the reason if it cannot be drawn.
    pub fn frame(&self, outline: &[P2], curved: bool, height: f32) -> Result<Frame, String> {
        self.check()?;
        if outline.len() != 4 {
            return Err("furniture needs a 4-point footprint".into());
        }
        if !outline.iter().flatten().all(|v| v.is_finite()) {
            return Err("the footprint has coordinates that are not numbers".into());
        }
        if !(height.is_finite() && (HEIGHT_M.0..=HEIGHT_M.1).contains(&height)) {
            return Err(format!(
                "furniture needs a `height` within {}..={} m",
                HEIGHT_M.0, HEIGHT_M.1
            ));
        }
        let dist = |p: P2, q: P2| (q[0] - p[0]).hypot(q[1] - p[1]);
        let centre = [
            outline.iter().map(|p| p[0]).sum::<f32>() / 4.0,
            outline.iter().map(|p| p[1]).sum::<f32>() / 4.0,
        ];
        let side = |what: &str, v: f32| {
            if (SIDE_M.0..=SIDE_M.1).contains(&v) {
                Ok(v)
            } else {
                Err(format!(
                    "the piece's {what} must be within {}..={} m",
                    SIDE_M.0, SIDE_M.1
                ))
            }
        };
        if curved {
            if !self.kind.can_be_round() {
                return Err(format!(
                    "a {} needs a rectangular footprint: only a table, a side table and a parasol can be round",
                    self.kind.name()
                ));
            }
            let (d02, d13) = (dist(outline[0], outline[2]), dist(outline[1], outline[3]));
            if (d02 - d13).abs() > 0.02 * d02.max(d13) + 1e-3 {
                return Err("a round piece needs a circular footprint (`furniture.set` with `round`, or `shape.add_circle`)".into());
            }
            let diameter = side("diameter", (d02 + d13) / 2.0)?;
            return Ok(Frame {
                centre,
                ex: [1.0, 0.0],
                ez: [0.0, 1.0],
                length: diameter,
                depth: diameter,
                height,
                round: true,
            });
        }
        let (a, b, c, d) = (outline[0], outline[1], outline[2], outline[3]);
        let (length, depth) = (dist(a, b), dist(b, c));
        let dot = (b[0] - a[0]) * (c[0] - b[0]) + (b[1] - a[1]) * (c[1] - b[1]);
        if length < 1e-4
            || depth < 1e-4
            || (dist(c, d) - length).abs() > 0.02
            || (dist(d, a) - depth).abs() > 0.02
            || (dot / (length * depth)).abs() > 0.02
        {
            return Err("furniture needs a rectangular footprint".into());
        }
        let (length, depth) = (side("length", length)?, side("depth", depth)?);
        Ok(Frame {
            centre,
            ex: [(b[0] - a[0]) / length, (b[1] - a[1]) / length],
            ez: [(c[0] - b[0]) / depth, (c[1] - b[1]) / depth],
            length,
            depth,
            height,
            round: false,
        })
    }
}

/// Where a piece stands and how big it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub centre: P2,
    /// Unit direction of the length (`+u`) on the plan.
    pub ex: P2,
    /// Unit direction of the depth (`+v`, the front) on the plan.
    pub ez: P2,
    pub length: f32,
    pub depth: f32,
    pub height: f32,
    /// A circular footprint (`length` is the diameter).
    pub round: bool,
}

impl Frame {
    /// `[length, depth, height]` in metres rounded to a tenth of a millimetre, as reported.
    pub fn dims(&self) -> [f64; 3] {
        [self.length, self.depth, self.height].map(|v| (f64::from(v) * 1e4).round() / 1e4)
    }

    /// The turn of the piece, counter-clockwise from above in degrees.
    pub fn rotation_deg(&self) -> f32 {
        (-self.ex[1]).atan2(self.ex[0]).to_degrees()
    }
}

/// The outline of a `length` by `depth` piece centred on `centre`, turned `rotation_deg`
/// counter-clockwise seen from above (-Z north, +X east): the back-left, back-right, front-right
/// and front-left corners.
pub fn rect_outline(centre: P2, rotation_deg: f32, length: f32, depth: f32) -> Vec<P2> {
    let t = rotation_deg.to_radians();
    let (ex, ez) = ([t.cos(), -t.sin()], [t.sin(), t.cos()]);
    let at = |u: f32, v: f32| {
        [
            centre[0] + ex[0] * u + ez[0] * v,
            centre[1] + ex[1] * u + ez[1] * v,
        ]
    };
    let (hl, hd) = (length / 2.0, depth / 2.0);
    vec![at(-hl, -hd), at(hl, -hd), at(hl, hd), at(-hl, hd)]
}

/// The four points of a circle of `diameter` round `centre` and the arcs that make them one
/// (as `shape.add_circle` draws it): east, south, west, north.
pub fn round_outline(centre: P2, diameter: f32) -> (Vec<P2>, crate::curve::Curves) {
    let r = diameter / 2.0;
    let [cx, cz] = centre;
    let arc = crate::curve::Curve::Arc {
        sagitta_m: r * (1.0 - std::f32::consts::FRAC_1_SQRT_2),
    };
    (
        vec![[cx + r, cz], [cx, cz + r], [cx - r, cz], [cx, cz - r]],
        (0..4).map(|i| (i, arc)).collect(),
    )
}

// --- Drawing --------------------------------------------------------------------------------

/// Draws boxes, prisms and planes in a piece's local coordinates `(u, y, v)` into the sink.
struct Pen<'a> {
    s: &'a mut dyn Sink,
    fr: &'a Frame,
    base: f32,
    faces: &'static [&'static str],
}

impl Pen<'_> {
    fn w(&self, u: f32, y: f32, v: f32) -> P3 {
        let (c, ex, ez) = (self.fr.centre, self.fr.ex, self.fr.ez);
        [
            c[0] + ex[0] * u + ez[0] * v,
            self.base + y,
            c[1] + ex[1] * u + ez[1] * v,
        ]
    }

    fn face(&self, name: &str) -> usize {
        self.faces
            .iter()
            .position(|f| *f == name)
            .unwrap_or_else(|| panic!("{name} is not a face of this piece"))
    }

    fn quad(&mut self, face: usize, v: [P3; 4], outward: P3) {
        self.s.tri(face, [v[0], v[1], v[2]], outward);
        self.s.tri(face, [v[0], v[2], v[3]], outward);
    }

    /// A convex solid from a bottom ring `c[0..4]` and a top ring `c[4..8]` (local points, the
    /// same order round), its sides facing out. `bottom`: also its bottom face.
    fn hexa(&mut self, face: &str, c: [P3; 8], bottom: bool) {
        let face = self.face(face);
        let p: Vec<P3> = c.iter().map(|q| self.w(q[0], q[1], q[2])).collect();
        let mid = {
            let mut m = [0.0; 3];
            for q in &p {
                for k in 0..3 {
                    m[k] += q[k] / 8.0;
                }
            }
            m
        };
        let quads: [[usize; 4]; 6] = [
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [1, 2, 6, 5],
            [2, 3, 7, 6],
            [3, 0, 4, 7],
            [0, 1, 2, 3],
        ];
        for (i, q) in quads.iter().enumerate() {
            if i == 5 && !bottom {
                continue;
            }
            let v = q.map(|k| p[k]);
            let centre: P3 = std::array::from_fn(|k| (v[0][k] + v[1][k] + v[2][k] + v[3][k]) / 4.0);
            let outward = [centre[0] - mid[0], centre[1] - mid[1], centre[2] - mid[2]];
            self.quad(face, v, outward);
        }
    }

    /// A box over `u0..u1`, `y0..y1`, `v0..v1`; one thinner than a millimetre is left out.
    fn cuboid(&mut self, face: &str, u: (f32, f32), y: (f32, f32), v: (f32, f32), bottom: bool) {
        if u.1 - u.0 < 1e-3 || y.1 - y.0 < 1e-3 || v.1 - v.0 < 1e-3 {
            return;
        }
        let ring = |y: f32| [[u.0, y, v.0], [u.1, y, v.0], [u.1, y, v.1], [u.0, y, v.1]];
        let (b, t) = (ring(y.0), ring(y.1));
        self.hexa(
            face,
            [b[0], b[1], b[2], b[3], t[0], t[1], t[2], t[3]],
            bottom,
        );
    }

    /// A prism with `n` sides standing at `(u, v)` over `y0..y1`, caps included.
    fn prism(&mut self, face: &str, at: (f32, f32), r: f32, y: (f32, f32), n: usize) {
        if r < 5e-4 || y.1 - y.0 < 1e-3 {
            return;
        }
        let face = self.face(face);
        let ring = |k: usize| {
            let a = TAU * k as f32 / n as f32 + PI / n as f32 / 2.0;
            (at.0 + r * a.cos(), at.1 + r * a.sin())
        };
        let c0 = self.w(at.0, y.0, at.1);
        let c1 = self.w(at.0, y.1, at.1);
        for k in 0..n {
            let (a, b) = (ring(k), ring(k + 1));
            let (a0, b0) = (self.w(a.0, y.0, a.1), self.w(b.0, y.0, b.1));
            let (a1, b1) = (self.w(a.0, y.1, a.1), self.w(b.0, y.1, b.1));
            let mid = self.w((a.0 + b.0) / 2.0, y.0, (a.1 + b.1) / 2.0);
            let outward = [mid[0] - c0[0], 0.0, mid[2] - c0[2]];
            self.quad(face, [a0, b0, b1, a1], outward);
            self.s.tri(face, [c1, a1, b1], [0.0, 1.0, 0.0]);
            self.s.tri(face, [c0, a0, b0], [0.0, -1.0, 0.0]);
        }
    }

    /// Four legs of `t` square at the corners, `inset` in from the footprint's edge, standing
    /// from the ground up to `top` (sunk `sink` into what they carry).
    fn legs(&mut self, face: &str, t: f32, inset: f32, top: f32, sink: f32) {
        let (hl, hd) = (self.fr.length / 2.0, self.fr.depth / 2.0);
        for su in [-1.0, 1.0] {
            for sv in [-1.0, 1.0] {
                let (u, v) = (su * (hl - inset), sv * (hd - inset));
                self.cuboid(
                    face,
                    (u.min(u - su * t), u.max(u - su * t)),
                    (0.0, top + sink),
                    (v.min(v - sv * t), v.max(v - sv * t)),
                    true,
                );
            }
        }
    }
}

/// The width of a sofa's arms (metres).
fn arm_width(length: f32) -> f32 {
    (length * 0.12).min(0.16)
}

/// Draws the piece `f` standing in `fr` on `base` into `s`; faces are the indices of
/// `f.faces()`.
pub(crate) fn mesh(s: &mut dyn Sink, f: &Furniture, fr: &Frame, base: f32) {
    let mut pen = Pen {
        s,
        fr,
        base,
        faces: f.faces(),
    };
    match f.kind {
        FurnitureKind::Table => table(&mut pen, f, 0.04, 0.06, 0.03, false),
        FurnitureKind::SideTable => table(&mut pen, f, 0.03, 0.035, 0.03, true),
        FurnitureKind::Chair => chair(&mut pen),
        FurnitureKind::Bench => bench(&mut pen, f.back),
        FurnitureKind::Sofa | FurnitureKind::Armchair => sofa(&mut pen, f),
        FurnitureKind::Lounger => lounger(&mut pen, f.back_angle()),
        FurnitureKind::Parasol => parasol(&mut pen, f.open),
        FurnitureKind::Cabinet => cabinet(&mut pen, f),
        FurnitureKind::Bed => bed(&mut pen),
    }
}

/// A table or side table: a top and four legs, or a central pedestal (always for a round one
/// unless `pedestal` says no).
fn table(p: &mut Pen, f: &Furniture, top_t: f32, leg_t: f32, inset: f32, shelf: bool) {
    let (l, d, h) = (p.fr.length, p.fr.depth, p.fr.height);
    let tt = top_t.min(h * 0.2);
    let pedestal = f.pedestal.unwrap_or(p.fr.round);
    if p.fr.round {
        p.prism("top", (0.0, 0.0), l / 2.0, (h - tt, h), 40);
    } else {
        p.cuboid(
            "top",
            (-l / 2.0, l / 2.0),
            (h - tt, h),
            (-d / 2.0, d / 2.0),
            true,
        );
    }
    let into = tt * 0.5;
    if pedestal {
        let foot = (l.min(d) * 0.55).min(0.35);
        let col = (l.min(d) * 0.07).clamp(0.02, 0.07);
        let foot_h = 0.03_f32.min(h * 0.1);
        if p.fr.round {
            p.prism("frame", (0.0, 0.0), foot, (0.0, foot_h), 24);
            p.prism("frame", (0.0, 0.0), col, (foot_h * 0.5, h - tt + into), 16);
        } else {
            let foot_d = (d * 0.8).min(0.6);
            p.cuboid(
                "frame",
                (-foot, foot),
                (0.0, foot_h),
                (-foot_d / 2.0, foot_d / 2.0),
                true,
            );
            p.cuboid(
                "frame",
                (-col, col),
                (foot_h * 0.5, h - tt + into),
                (-col, col),
                true,
            );
        }
        return;
    }
    let t = leg_t.min(l.min(d) * 0.2);
    if p.fr.round {
        // Four legs on a ring just inside the edge.
        let ring = (l / 2.0 - inset - t / 2.0).max(0.0);
        for k in 0..4 {
            let a = FRAC_PI_2 * k as f32 + PI / 4.0;
            let (u, v) = (ring * a.cos(), ring * a.sin());
            p.cuboid(
                "frame",
                (u - t / 2.0, u + t / 2.0),
                (0.0, h - tt + into),
                (v - t / 2.0, v + t / 2.0),
                true,
            );
        }
    } else {
        p.legs("frame", t, inset.min(l.min(d) * 0.2), h - tt, into);
        if shelf && h > 0.3 {
            // A low shelf between the legs.
            let (s_u, s_v) = (l / 2.0 - inset - 0.002, d / 2.0 - inset - 0.002);
            p.cuboid(
                "frame",
                (-s_u, s_u),
                (h * 0.28, h * 0.28 + 0.02),
                (-s_v, s_v),
                true,
            );
        }
    }
}

/// A chair: a seat, a back panel between rear legs that rise to the full height, and four legs.
fn chair(p: &mut Pen) {
    let (l, d, h) = (p.fr.length, p.fr.depth, p.fr.height);
    let sh = 0.45_f32.min(h * 0.53);
    let st = 0.04_f32.min(sh * 0.2);
    let t = 0.04_f32.min(l * 0.2);
    let inset = 0.01;
    p.cuboid(
        "top",
        (-l / 2.0, l / 2.0),
        (sh - st, sh),
        (-d / 2.0, d / 2.0),
        true,
    );
    for su in [-1.0_f32, 1.0] {
        let u = su * (l / 2.0 - inset);
        let us = (u.min(u - su * t), u.max(u - su * t));
        // Front leg to the seat, rear leg right up to the top of the back.
        let v = d / 2.0 - inset;
        p.cuboid("frame", us, (0.0, sh - st * 0.5), (v - t, v), true);
        let v = -d / 2.0 + inset;
        p.cuboid("frame", us, (0.0, h), (v, v + t), true);
    }
    let inner = l / 2.0 - inset - t + 0.005;
    let y0 = (sh + 0.1).min(h - 0.06);
    p.cuboid(
        "top",
        (-inner, inner),
        (y0, h - 0.03_f32.min((h - y0) * 0.5)),
        (-d / 2.0 + inset + 0.005, -d / 2.0 + inset + t - 0.005),
        true,
    );
}

/// A bench: a seat slab on four legs, with or without a back panel between taller rear legs.
fn bench(p: &mut Pen, back: bool) {
    let (l, d, h) = (p.fr.length, p.fr.depth, p.fr.height);
    let sh = if back { 0.45_f32.min(h * 0.55) } else { h };
    let st = 0.05_f32.min(sh * 0.2);
    let t = 0.06_f32.min(l.min(d) * 0.2);
    let inset = 0.02;
    p.cuboid(
        "top",
        (-l / 2.0, l / 2.0),
        (sh - st, sh),
        (-d / 2.0, d / 2.0),
        true,
    );
    for su in [-1.0_f32, 1.0] {
        let u = su * (l / 2.0 - inset);
        let us = (u.min(u - su * t), u.max(u - su * t));
        let v = d / 2.0 - inset;
        p.cuboid("frame", us, (0.0, sh - st * 0.5), (v - t, v), true);
        let v = -d / 2.0 + inset;
        let top = if back { h } else { sh - st * 0.5 };
        p.cuboid("frame", us, (0.0, top), (v, v + t), true);
    }
    if back {
        let inner = l / 2.0 - inset - t + 0.005;
        let y0 = (sh + 0.12).min(h - 0.06);
        p.cuboid(
            "top",
            (-inner, inner),
            (y0, h - 0.02),
            (-d / 2.0 + inset + 0.005, -d / 2.0 + inset + t - 0.005),
            true,
        );
    }
}

/// A sofa or armchair: feet, a base with a back shell and two arms, seat cushions between the
/// arms and as many back cushions standing against the back.
fn sofa(p: &mut Pen, f: &Furniture) {
    let (l, d, h) = (p.fr.length, p.fr.depth, p.fr.height);
    let foot_h = 0.1_f32.min(h * 0.15);
    let seat = h * 0.4;
    let aw = arm_width(l);
    let back_t = 0.16_f32.min(d * 0.2);
    let back_c = 0.2_f32.min(d * 0.25);
    let arm_top = seat + (h - seat) * 0.55;
    p.legs(
        "frame",
        0.05_f32.min(l.min(d) * 0.1),
        0.04_f32.min(l.min(d) * 0.1),
        foot_h,
        0.01,
    );
    p.cuboid(
        "top",
        (-l / 2.0, l / 2.0),
        (foot_h, seat),
        (-d / 2.0, d / 2.0),
        true,
    );
    // The back shell and the arms stand on the base, their sides flush with it.
    p.cuboid(
        "top",
        (-l / 2.0, l / 2.0),
        (seat, h * 0.85),
        (-d / 2.0, -d / 2.0 + back_t),
        false,
    );
    for su in [-1.0_f32, 1.0] {
        let u = su * l / 2.0;
        p.cuboid(
            "top",
            (u.min(u - su * aw), u.max(u - su * aw)),
            (seat, arm_top),
            (-d / 2.0 + back_t, d / 2.0),
            false,
        );
    }
    let n = f.cushion_count(l);
    let inner = l - 2.0 * aw;
    let gap = 0.01_f32.min(inner / n as f32 * 0.1);
    let seat_t = 0.14_f32.min((h - seat) * 0.4);
    for i in 0..n {
        let u0 = -inner / 2.0 + inner * i as f32 / n as f32 + gap;
        let u1 = -inner / 2.0 + inner * (i + 1) as f32 / n as f32 - gap;
        p.cuboid(
            "cushion",
            (u0, u1),
            (seat, seat + seat_t),
            (-d / 2.0 + back_t + back_c, d / 2.0),
            false,
        );
        p.cuboid(
            "cushion",
            (u0, u1),
            (seat + seat_t, h),
            (-d / 2.0 + back_t, -d / 2.0 + back_t + back_c),
            false,
        );
    }
}

/// A sun lounger: a frame on four legs, a seat pad and a back rest tilted `back_deg` above flat,
/// hinged where the pad ends. The head is at `-u`.
fn lounger(p: &mut Pen, back_deg: f32) {
    let (l, d, h) = (p.fr.length, p.fr.depth, p.fr.height);
    let sh = 0.32_f32.min(h * 0.45);
    let rail = 0.06_f32.min(sh * 0.4);
    let side = 0.05_f32.min(d * 0.15);
    let a = back_deg.to_radians();
    let (sa, ca) = (a.sin(), a.cos());
    let (board_t, pad_t) = (0.02, 0.06);
    let thick = board_t + pad_t;
    let mut lb = 0.36 * l;
    if sa > 1e-3 {
        lb = lb.min((h - sh - thick * ca) / sa);
    }
    let lb = lb.max(0.05);
    let uj = -l / 2.0 + lb * ca;
    // Side rails the full length and a board between them.
    for sv in [-1.0_f32, 1.0] {
        let v = sv * d / 2.0;
        p.cuboid(
            "frame",
            (-l / 2.0, l / 2.0),
            (sh - rail, sh),
            (v.min(v - sv * side), v.max(v - sv * side)),
            true,
        );
    }
    p.cuboid(
        "frame",
        (-l / 2.0, l / 2.0),
        (sh - 0.02, sh),
        (-d / 2.0 + side, d / 2.0 - side),
        true,
    );
    // Legs under the rails near both ends.
    let t = 0.05_f32.min(d * 0.15);
    for u in [-l / 2.0 + 0.08, l / 2.0 - 0.08 - t] {
        for sv in [-1.0_f32, 1.0] {
            let v = sv * (d / 2.0 - 0.01);
            p.cuboid(
                "frame",
                (u, u + t),
                (0.0, sh - rail * 0.5),
                (v.min(v - sv * t), v.max(v - sv * t)),
                true,
            );
        }
    }
    // The seat pad, hinge to foot.
    p.cuboid(
        "cushion",
        (uj, l / 2.0),
        (sh, sh + pad_t),
        (-d / 2.0 + 0.01, d / 2.0 - 0.01),
        false,
    );
    // The back rest: a board and a pad above it, along `dir` up and towards the head from the
    // hinge, `n` the way its top faces.
    let dir = [-ca, sa];
    let n = [sa, ca];
    let at = |s: f32, t: f32, v: f32| [uj + dir[0] * s + n[0] * t, sh + dir[1] * s + n[1] * t, v];
    let (v0, v1) = (-d / 2.0 + 0.01, d / 2.0 - 0.01);
    for (face, t0, t1) in [("frame", 0.0, board_t), ("cushion", board_t, thick)] {
        p.hexa(
            face,
            [
                at(0.0, t0, v0),
                at(lb, t0, v0),
                at(lb, t0, v1),
                at(0.0, t0, v1),
                at(0.0, t1, v0),
                at(lb, t1, v0),
                at(lb, t1, v1),
                at(0.0, t1, v1),
            ],
            face == "frame",
        );
    }
}

/// A parasol: a base disc, a pole and an octagonal canopy, open (a shallow umbrella) or folded
/// (a narrow cone round the pole).
fn parasol(p: &mut Pen, open: bool) {
    let (l, d, h) = (p.fr.length, p.fr.depth, p.fr.height);
    let r = l.min(d) / 2.0;
    let base_r = (r * 0.5).clamp(0.05, 0.3);
    let base_h = 0.05_f32.min(h * 0.05);
    p.prism("frame", (0.0, 0.0), base_r, (0.0, base_h), 24);
    p.prism(
        "frame",
        (0.0, 0.0),
        0.025_f32.min(r * 0.2),
        (base_h * 0.5, h - 0.01),
        12,
    );
    let (rim_r, rim_y) = if open {
        (r, (h - r * 0.27).max(h * 0.6))
    } else {
        ((r * 0.08).max(0.03), h - (h * 0.45).min(1.1))
    };
    let canopy = p.face("canopy");
    let apex = p.w(0.0, h, 0.0);
    let ring: Vec<P3> = (0..8)
        .map(|k| {
            let a = TAU * k as f32 / 8.0 + PI / 8.0;
            p.w(rim_r * a.cos(), rim_y, rim_r * a.sin())
        })
        .collect();
    for k in 0..8 {
        let tri = [apex, ring[k], ring[(k + 1) % 8]];
        p.s.tri(canopy, tri, [0.0, 1.0, 0.0]);
        p.s.tri(canopy, tri, [0.0, -1.0, 0.0]);
    }
}

/// A cabinet: a plinth, a body and a front of separate panels, drawers over doors.
fn cabinet(p: &mut Pen, f: &Furniture) {
    let (l, d, h) = (p.fr.length, p.fr.depth, p.fr.height);
    let plinth = 0.08_f32.min(h * 0.15);
    let panel_t = 0.018_f32.min(d * 0.3);
    let recess = 0.02_f32.min(d * 0.1);
    p.cuboid(
        "frame",
        (-l / 2.0 + recess, l / 2.0 - recess),
        (0.0, plinth + 0.01),
        (-d / 2.0 + recess, d / 2.0 - recess),
        true,
    );
    p.cuboid(
        "top",
        (-l / 2.0, l / 2.0),
        (plinth, h),
        (-d / 2.0, d / 2.0 - panel_t),
        true,
    );
    // The front, a panel apiece with a hairline groove between.
    let (margin, groove) = (0.006, 0.004);
    let front = (d / 2.0 - panel_t, d / 2.0);
    let (y0, y1) = (plinth + margin, h - margin);
    let rows = f.drawers as usize;
    let drawer_h = (0.18_f32).min((y1 - y0) * 0.25);
    let doors_y1 = y1 - rows as f32 * (drawer_h + groove);
    for r in 0..rows {
        let top = y1 - r as f32 * (drawer_h + groove);
        p.cuboid(
            "frame",
            (-l / 2.0 + margin, l / 2.0 - margin),
            (top - drawer_h, top),
            front,
            true,
        );
    }
    let n = f.door_count(l);
    let span = l - 2.0 * margin;
    let w = (span - groove * (n as f32 - 1.0)) / n as f32;
    for i in 0..n {
        let u0 = -l / 2.0 + margin + i as f32 * (w + groove);
        p.cuboid("frame", (u0, u0 + w), (y0, doors_y1), front, true);
    }
}

/// An indoor bed: legs, a frame, a mattress, pillows and a headboard at `-u`.
fn bed(p: &mut Pen) {
    let (l, d, h) = (p.fr.length, p.fr.depth, p.fr.height);
    let leg_h = (0.1_f32).min(h * 0.12);
    let frame_top = (0.3_f32).min(h * 0.35);
    let mat_t = (0.22_f32).min(h * 0.25);
    let head_t = 0.06_f32.min(l * 0.1);
    let t = 0.06_f32.min(d * 0.1);
    p.legs("frame", t, 0.02, leg_h, 0.01);
    p.cuboid(
        "frame",
        (-l / 2.0, l / 2.0),
        (leg_h, frame_top),
        (-d / 2.0, d / 2.0),
        true,
    );
    p.cuboid(
        "top",
        (-l / 2.0, -l / 2.0 + head_t),
        (leg_h, h),
        (-d / 2.0, d / 2.0),
        true,
    );
    let (m_u0, m_u1) = (-l / 2.0 + head_t, l / 2.0 - 0.03);
    let m_v = d / 2.0 - 0.03;
    p.cuboid(
        "cushion",
        (m_u0, m_u1),
        (frame_top, frame_top + mat_t),
        (-m_v, m_v),
        false,
    );
    // Pillows at the head: two side by side on a double bed, one on a single.
    let n = if d >= 1.2 { 2 } else { 1 };
    let pw = ((2.0 * m_v - 0.1) / n as f32 - 0.05).min(0.6);
    let (pu, pt) = (0.35_f32.min((m_u1 - m_u0) * 0.4), 0.12);
    let top = frame_top + mat_t;
    for i in 0..n {
        let c = (i as f32 - (n as f32 - 1.0) / 2.0) * (pw + 0.05);
        p.cuboid(
            "cushion",
            (m_u0 + 0.03, m_u0 + 0.03 + pu),
            (top, top + pt),
            (c - pw / 2.0, c + pw / 2.0),
            false,
        );
    }
}

// --- Sets -----------------------------------------------------------------------------------

/// The groups `furniture.add_set` places.
pub const SETS: [&str; 3] = ["dining", "lounge", "sunbathing"];
/// The most seats a dining set has.
pub const MAX_SEATS: usize = 12;
/// The seats a dining set starts with.
pub const DEFAULT_SEATS: usize = 6;

/// One piece of a set, placed relative to the set's centre (set turn 0): `position` as
/// `[u, v]` along the set's own axes (east and south at turn 0), `rotation_deg` its own turn.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedPiece {
    pub kind: FurnitureKind,
    pub name: &'static str,
    pub position: P2,
    pub rotation_deg: f32,
    /// `[length, depth, height]`.
    pub size: [f32; 3],
}

/// The pieces of the set `set` (see [`SETS`]); `seats` only counts for a dining set (2..=12).
/// The pieces at the front of the set (+v) face -v; turn a piece by `rotation_deg` and its front
/// is `(sin, cos)` of that angle on the plan.
pub fn set_pieces(set: &str, seats: usize) -> Result<Vec<PlannedPiece>, String> {
    let sized = |kind: FurnitureKind| default_size(kind, true, false);
    let piece = |kind, name, position, rotation_deg, size| PlannedPiece {
        kind,
        name,
        position,
        rotation_deg,
        size,
    };
    match set {
        "dining" => {
            if !(2..=MAX_SEATS).contains(&seats) {
                return Err(format!("`seats` must be 2..={MAX_SEATS}"));
            }
            let per_side = seats.div_ceil(2);
            let length = (per_side as f32 * 0.6 + 0.3).max(1.0);
            let depth = 0.9;
            let table = [length, depth, 0.75];
            let chair = sized(FurnitureKind::Chair);
            // Pulled up so the seat tucks 0.1 m under the table's edge.
            let z = depth / 2.0 + chair[1] / 2.0 - 0.1;
            let mut out = vec![piece(
                FurnitureKind::Table,
                "Dining table",
                [0.0, 0.0],
                0.0,
                table,
            )];
            for (count, side) in [(per_side, -1.0_f32), (seats - per_side, 1.0)] {
                for i in 0..count {
                    let u = (i as f32 - (count as f32 - 1.0) / 2.0) * 0.6;
                    // The north side faces +v (turn 0), the south side faces -v (turn 180).
                    let turn = if side < 0.0 { 0.0 } else { 180.0 };
                    out.push(piece(
                        FurnitureKind::Chair,
                        "Dining chair",
                        [u, side * z],
                        turn,
                        chair,
                    ));
                }
            }
            Ok(out)
        }
        "lounge" => {
            let sofa = sized(FurnitureKind::Sofa);
            let arm = sized(FurnitureKind::Armchair);
            Ok(vec![
                piece(FurnitureKind::Sofa, "Sofa", [0.0, -1.3], 0.0, sofa),
                piece(FurnitureKind::Armchair, "Armchair", [-1.3, 0.0], 90.0, arm),
                piece(FurnitureKind::Armchair, "Armchair", [1.3, 0.0], 270.0, arm),
                piece(
                    FurnitureKind::SideTable,
                    "Coffee table",
                    [0.0, 0.0],
                    0.0,
                    [0.7, 0.7, 0.4],
                ),
            ])
        }
        "sunbathing" => {
            let lounger = sized(FurnitureKind::Lounger);
            Ok(vec![
                // Heads to the north (-v): the head is at -u, so u points south.
                piece(
                    FurnitureKind::Lounger,
                    "Sun lounger",
                    [-0.75, 0.0],
                    270.0,
                    lounger,
                ),
                piece(
                    FurnitureKind::Lounger,
                    "Sun lounger",
                    [0.75, 0.0],
                    270.0,
                    lounger,
                ),
                piece(
                    FurnitureKind::SideTable,
                    "Side table",
                    [0.0, 0.0],
                    0.0,
                    [0.45, 0.45, 0.5],
                ),
                piece(
                    FurnitureKind::Parasol,
                    "Parasol",
                    [0.0, -1.15],
                    0.0,
                    [2.6, 2.6, 2.4],
                ),
            ])
        }
        other => Err(format!("unknown set `{other}`; one of {}", SETS.join(", "))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::{Extrusion, Shape, shape_mesh};

    fn piece(
        f: Furniture,
        outline: Vec<P2>,
        curves: crate::curve::Curves,
        base: f32,
        h: f32,
    ) -> Shape {
        Shape::Extrusion(Extrusion {
            id: "S1".into(),
            name: "Piece".into(),
            layer: "shapes".into(),
            points: outline,
            curves,
            smooth: false,
            closed: true,
            base,
            height: h,
            thickness: 0.2,
            extensions: Default::default(),
            house: None,
            fence: None,
            stairs: None,
            raised_bed: None,
            furniture: Some(f),
        })
    }

    /// A piece of `kind` at its default size, centred on (3, -2), turned `rot` degrees.
    fn default_piece(kind: FurnitureKind, rot: f32) -> (Shape, [f32; 3]) {
        let f = Furniture::new(kind);
        let [l, d, h] = f.default_size(false);
        let shape = piece(
            f,
            rect_outline([3.0, -2.0], rot, l, d),
            Default::default(),
            0.2,
            h,
        );
        (shape, [l, d, h])
    }

    /// The vertices of `shape`'s mesh in the piece's own frame `(u, y, v)`, `y` above the base.
    fn local_points(shape: &Shape) -> Vec<P3> {
        let m = shape_mesh(shape).unwrap();
        let Shape::Extrusion(e) = shape else {
            unreachable!()
        };
        let f = e.furniture.as_ref().unwrap();
        let fr = f.frame(&e.points, shape.is_curved(), e.height).unwrap();
        m.positions
            .iter()
            .map(|p| {
                let (dx, dz) = (p[0] - fr.centre[0], p[2] - fr.centre[1]);
                [
                    dx * fr.ex[0] + dz * fr.ex[1],
                    p[1] - e.base,
                    dx * fr.ez[0] + dz * fr.ez[1],
                ]
            })
            .collect()
    }

    #[test]
    fn every_kind_builds_a_mesh_inside_its_footprint_and_height() {
        for kind in FurnitureKind::ALL {
            for rot in [0.0, 37.0, 180.0] {
                let (shape, [l, d, h]) = default_piece(kind, rot);
                let m = shape_mesh(&shape).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
                assert!(!m.triangles.is_empty(), "{kind:?}");
                let pts = local_points(&shape);
                let tol = 2e-3;
                for p in &pts {
                    assert!(
                        p[0].abs() <= l / 2.0 + tol
                            && p[2].abs() <= d / 2.0 + tol
                            && p[1] >= -tol
                            && p[1] <= h + tol,
                        "{kind:?} turned {rot}: {p:?} outside {l} x {d} x {h}"
                    );
                }
                // The piece reaches the ground and, but for a flat bench, near the full height.
                let top = pts.iter().map(|p| p[1]).fold(0.0, f32::max);
                let low = pts.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
                assert!(low.abs() < 1e-3, "{kind:?} floats {low} m");
                assert!(top > h * 0.85, "{kind:?} only {top} of {h} m high");
            }
        }
    }

    #[test]
    fn each_kind_has_the_faces_it_should_and_all_of_them_hold_triangles() {
        for kind in FurnitureKind::ALL {
            let (shape, _) = default_piece(kind, 0.0);
            let m = shape_mesh(&shape).unwrap();
            let names: Vec<_> = m.faces.iter().map(|f| f.name.as_str()).collect();
            let want: Vec<_> = kind.faces().iter().map(|f| format!("S1:{f}")).collect();
            assert_eq!(names, want, "{kind:?}");
            for (i, f) in m.faces.iter().enumerate() {
                assert!(
                    m.triangle_faces.iter().any(|t| *t as usize == i),
                    "{kind:?} has nothing on {}",
                    f.name
                );
            }
        }
        // Spot checks of the spec.
        assert_eq!(FurnitureKind::Lounger.faces(), ["frame", "cushion"]);
        assert_eq!(FurnitureKind::Parasol.faces(), ["frame", "canopy"]);
        assert!(!FurnitureKind::Table.faces().contains(&"cushion"));
        assert!(FurnitureKind::Bed.faces().contains(&"cushion"));
    }

    fn face_triangles(shape: &Shape, face: &str) -> usize {
        let m = shape_mesh(shape).unwrap();
        let i = m
            .faces
            .iter()
            .position(|f| f.name == format!("S1:{face}"))
            .unwrap();
        m.triangle_faces
            .iter()
            .filter(|t| **t as usize == i)
            .count()
    }

    #[test]
    fn sofa_cushions_grow_with_the_length() {
        let sofa = |l: f32| {
            let f = Furniture::new(FurnitureKind::Sofa);
            piece(
                f,
                rect_outline([0.0, 0.0], 0.0, l, 0.9),
                Default::default(),
                0.0,
                0.8,
            )
        };
        let counts: Vec<usize> = [1.4, 2.2, 3.0]
            .iter()
            .map(|l| Furniture::new(FurnitureKind::Sofa).cushion_count(*l))
            .collect();
        assert_eq!(counts, [2, 3, 4]);
        let tris: Vec<usize> = [1.4, 2.2, 3.0]
            .iter()
            .map(|l| face_triangles(&sofa(*l), "cushion"))
            .collect();
        assert!(tris[0] < tris[1] && tris[1] < tris[2], "{tris:?}");
        // An armchair is a one-seat sofa; a given count wins.
        let arm = Furniture::new(FurnitureKind::Armchair);
        assert_eq!(arm.cushion_count(0.9), 1);
        let mut four = Furniture::new(FurnitureKind::Sofa);
        four.cushions = Some(4);
        assert_eq!(four.cushion_count(1.0), 4);
    }

    #[test]
    fn a_round_table_has_a_circular_footprint_and_a_pedestal() {
        let (points, curves) = round_outline([1.0, 1.0], 1.2);
        let f = Furniture::new(FurnitureKind::Table);
        let shape = piece(f.clone(), points.clone(), curves, 0.0, 0.75);
        let fr = f.frame(&points, true, 0.75).unwrap();
        assert!(fr.round && (fr.length - 1.2).abs() < 1e-5);
        assert_eq!(fr.centre, [1.0, 1.0]);
        let pts = local_points(&shape);
        let reach = pts.iter().map(|p| p[0].hypot(p[2])).fold(0.0, f32::max);
        assert!((reach - 0.6).abs() < 2e-3, "{reach}");
        // A pedestal: the frame has no leg under the edge.
        let low_far = pts
            .iter()
            .filter(|p| p[1] < 0.5 && p[0].hypot(p[2]) > 0.4)
            .count();
        assert_eq!(low_far, 0);
        // A chair cannot be round.
        let chair = Furniture::new(FurnitureKind::Chair);
        let err = chair.frame(&points, true, 0.85).unwrap_err();
        assert!(err.contains("rectangular"), "{err}");
        // With four legs a round table has them near the edge.
        let mut legs = f;
        legs.pedestal = Some(false);
        let shape = piece(legs, points, round_outline([1.0, 1.0], 1.2).1, 0.0, 0.75);
        let far = local_points(&shape)
            .iter()
            .filter(|p| p[1] < 0.5 && p[0].hypot(p[2]) > 0.4)
            .count();
        assert!(far > 0);
    }

    #[test]
    fn the_lounger_back_follows_its_angle_within_the_footprint() {
        let top_at = |deg: f32| {
            let mut f = Furniture::new(FurnitureKind::Lounger);
            f.back_deg = Some(deg);
            let shape = piece(
                f,
                rect_outline([0.0, 0.0], 0.0, 1.95, 0.7),
                Default::default(),
                0.0,
                0.85,
            );
            let pts = local_points(&shape);
            assert!(
                pts.iter()
                    .all(|p| p[0] >= -1.95 / 2.0 - 2e-3 && p[1] <= 0.85 + 2e-3)
            );
            pts.iter().map(|p| p[1]).fold(0.0, f32::max)
        };
        assert!(top_at(10.0) < top_at(40.0) && top_at(40.0) <= top_at(75.0) + 1e-4);
        assert!(top_at(0.0) < 0.5);
        let mut bad = Furniture::new(FurnitureKind::Lounger);
        bad.back_deg = Some(95.0);
        assert!(bad.check().is_err());
    }

    #[test]
    fn a_parasol_folded_is_narrow_and_open_reaches_its_radius() {
        let reach = |open: bool| {
            let mut f = Furniture::new(FurnitureKind::Parasol);
            f.open = open;
            let shape = piece(
                f,
                rect_outline([0.0, 0.0], 0.0, 3.0, 3.0),
                Default::default(),
                0.0,
                2.4,
            );
            local_points(&shape)
                .iter()
                .map(|p| p[0].hypot(p[2]))
                .fold(0.0, f32::max)
        };
        // The base disc is 0.3 across; the open canopy 1.5.
        assert!((reach(true) - 1.5).abs() < 2e-3);
        assert!(reach(false) < 0.35);
    }

    #[test]
    fn a_bench_with_or_without_a_back_and_a_cabinet_with_doors() {
        let bench = |back: bool| {
            let mut f = Furniture::new(FurnitureKind::Bench);
            f.back = back;
            let [l, d, h] = f.default_size(false);
            let shape = piece(
                f,
                rect_outline([0.0, 0.0], 0.0, l, d),
                Default::default(),
                0.0,
                h,
            );
            let top = local_points(&shape)
                .iter()
                .map(|p| p[1])
                .fold(0.0, f32::max);
            (top, h)
        };
        let (with, hw) = bench(true);
        let (without, hn) = bench(false);
        assert!((with - hw).abs() < 1e-3 && (without - hn).abs() < 1e-3);
        assert!(hw > hn);
        // More doors and drawers, more triangles on the frame.
        let cabinet = |doors: Option<u32>, drawers: u32| {
            let mut f = Furniture::new(FurnitureKind::Cabinet);
            f.doors = doors;
            f.drawers = drawers;
            face_triangles(
                &piece(
                    f,
                    rect_outline([0.0, 0.0], 0.0, 1.2, 0.45),
                    Default::default(),
                    0.0,
                    0.9,
                ),
                "frame",
            )
        };
        assert!(cabinet(Some(4), 0) > cabinet(Some(2), 0));
        assert!(cabinet(Some(2), 2) > cabinet(Some(2), 0));
    }

    #[test]
    fn the_outline_turns_counter_clockwise_and_the_frame_reads_it_back() {
        // Turned 90 degrees the length axis points north (-z) and the front east (+x).
        let o = rect_outline([5.0, 5.0], 90.0, 2.0, 1.0);
        let f = Furniture::new(FurnitureKind::Sofa)
            .frame(&o, false, 0.8)
            .unwrap();
        assert!(f.ex[0].abs() < 1e-5 && (f.ex[1] + 1.0).abs() < 1e-5);
        assert!((f.ez[0] - 1.0).abs() < 1e-5 && f.ez[1].abs() < 1e-5);
        assert!((f.rotation_deg() - 90.0).abs() < 1e-3);
        assert!((f.length - 2.0).abs() < 1e-5 && (f.depth - 1.0).abs() < 1e-5);
        // Not a rectangle, not numbers, not a usable height.
        let skew = vec![[0.0, 0.0], [1.0, 0.0], [1.5, 1.0], [0.0, 1.0]];
        assert!(
            Furniture::new(FurnitureKind::Sofa)
                .frame(&skew, false, 0.8)
                .is_err()
        );
        assert!(
            Furniture::new(FurnitureKind::Sofa)
                .frame(&o, false, 0.0)
                .is_err()
        );
        assert!(
            Furniture::new(FurnitureKind::Sofa)
                .frame(&o, false, f32::NAN)
                .is_err()
        );
    }

    #[test]
    fn serde_names_and_defaults() {
        let f: Furniture = serde_json::from_str(r#"{"kind": "side_table"}"#).unwrap();
        assert_eq!(f, Furniture::new(FurnitureKind::SideTable));
        // The defaults are not written.
        assert_eq!(
            serde_json::to_string(&f).unwrap(),
            r#"{"kind":"side_table"}"#
        );
        let mut g = Furniture::new(FurnitureKind::Bench);
        g.back = false;
        g.pedestal = Some(true);
        let back: Furniture = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(back, g);
        for k in FurnitureKind::ALL {
            assert_eq!(FurnitureKind::parse(k.name()), Some(k));
        }
        assert!(FurnitureKind::parse("throne").is_none());
    }

    fn front(p: &PlannedPiece) -> P2 {
        let t = p.rotation_deg.to_radians();
        [t.sin(), t.cos()]
    }

    #[test]
    fn a_dining_set_has_a_table_and_a_chair_for_each_seat_facing_it() {
        let set = set_pieces("dining", 6).unwrap();
        assert_eq!(
            set.iter()
                .filter(|p| p.kind == FurnitureKind::Table)
                .count(),
            1
        );
        let chairs: Vec<_> = set
            .iter()
            .filter(|p| p.kind == FurnitureKind::Chair)
            .collect();
        assert_eq!(chairs.len(), 6);
        let table = set.iter().find(|p| p.kind == FurnitureKind::Table).unwrap();
        for c in &chairs {
            let to = [
                table.position[0] - c.position[0],
                table.position[1] - c.position[1],
            ];
            let f = front(c);
            assert!(f[0] * to[0] + f[1] * to[1] > 0.1, "{c:?} faces away");
            // Seated at the table's edge: the seat's front is under it.
            assert!(c.position[1].abs() < table.size[1] / 2.0 + c.size[1] / 2.0);
            assert!(c.position[0].abs() <= table.size[0] / 2.0);
        }
        assert_eq!(set_pieces("dining", 2).unwrap().len(), 3);
        assert_eq!(set_pieces("dining", 5).unwrap().len(), 6);
        assert!(set_pieces("dining", 1).is_err() && set_pieces("dining", 13).is_err());
        assert!(set_pieces("picnic", 4).is_err());
    }

    #[test]
    fn the_lounge_and_sunbathing_sets_have_their_pieces() {
        let kinds = |set: &str| -> Vec<FurnitureKind> {
            set_pieces(set, DEFAULT_SEATS)
                .unwrap()
                .iter()
                .map(|p| p.kind)
                .collect()
        };
        use FurnitureKind::*;
        assert_eq!(kinds("lounge"), [Sofa, Armchair, Armchair, SideTable]);
        assert_eq!(kinds("sunbathing"), [Lounger, Lounger, SideTable, Parasol]);
        // The seating faces the middle.
        for p in set_pieces("lounge", 0)
            .unwrap()
            .iter()
            .filter(|p| matches!(p.kind, Sofa | Armchair))
        {
            let f = front(p);
            assert!(f[0] * -p.position[0] + f[1] * -p.position[1] > 0.5, "{p:?}");
        }
    }
}
