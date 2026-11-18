//! Terraces cut into the terrain: a level that moves the ground, held by a retaining wall.
//!
//! A terrace is a flat area (`Area::terrace`, not draped) whose floor is the area's `elevation`,
//! an absolute height. Inside its outline the terrace replaces the ground; the terrain's height
//! grid is not edited. Instead
//!
//! - the terrain's chunk meshes get the footprint clipped out (`terrain_chunk_mesh_holes`): the
//!   outline moved outwards by the wall's thickness ([`Terrace::hole`]), cut out of the triangles
//!   exactly, the outside keeping the original heights;
//! - the retaining wall fills the band between the outline and the hole and follows the ground
//!   along the edge ([`Terrace::stations`]): from the lower of the floor and the ground at that
//!   point up to the higher of the two, plus `coping_m` above the ground where the terrace is cut
//!   into it, or plus `parapet_m` above the floor where it is built up;
//! - every question about the height of the ground inside the outline answers with the floor
//!   ([`ground_at`], [`ground_fn`]), and a ray at the terrain misses where the hole is
//!   ([`terrain_raycast`]).
//!
//! The wall has a [`BedStyle`], drawn by the same builders a raised bed uses where it can: plain,
//! masonry with a coping, sleepers laid in courses, a corten plate, or a gabion (a stone wall in
//! a wire cage). Its faces are `<id>:wall` and `<id>:wall_rim`.

use crate::bed::{self, AreaEdge, Beam, BedStyle, RaisedBed};
use crate::house::Sink;
use crate::project::Project;
use crate::shape::{Shape, inside_outline, offset_outline, shoelace_f32};
use crate::terrain::Terrain;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

type P2 = [f32; 2];

/// Faces a terrace has after its top, in this order (`<id>:wall`, `<id>:wall_rim`).
pub const FACES: [&str; 2] = ["wall", "wall_rim"];

/// The terrain cell size to sample the ground by when there is no terrain.
pub const DEFAULT_CELL_M: f32 = 1.0;
/// How far a wall stands above the ground on a cut stretch when nothing else is asked (metres).
pub const DEFAULT_COPING_M: f32 = 0.1;
/// The tallest coping and parapet (metres).
pub const MAX_COPING_M: f32 = 1.0;
/// A wall taller than this needs an engineer's design (metres).
pub const ENGINEER_M: f32 = 1.2;
/// How far a wall is sunk under the lower of the floor and the ground, so the terrain that
/// meets it at a coarser level of detail leaves no gap (metres).
const SINK_M: f32 = 0.05;
/// Gabion bars: the pitch between them, their height and how far they stand off the face.
const BAR_PITCH_M: f32 = 0.15;
const BAR_H: f32 = 0.012;
const BAR_OUT: f32 = 0.008;

fn default_coping() -> f32 {
    DEFAULT_COPING_M
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_default_coping(v: &f32) -> bool {
    *v == DEFAULT_COPING_M
}

/// The retaining wall and cut of a terrace (`Area::terrace`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Terrace {
    /// The wall's style; plain is a solid timber wall.
    #[serde(default, skip_serializing_if = "BedStyle::is_plain")]
    pub style: BedStyle,
    /// How thick the wall is (a sleeper's thickness for sleepers, the plate's for corten); the
    /// style's own when absent. The wall stands outside the outline, so the floor keeps its size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thickness_m: Option<f32>,
    /// How far the wall's top stands above the ground where the terrace is cut into it.
    #[serde(default = "default_coping", skip_serializing_if = "is_default_coping")]
    pub coping_m: f32,
    /// How far the wall's top stands above the floor where the terrace is built up: a seat wall.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub parapet_m: f32,
}

impl Default for Terrace {
    fn default() -> Self {
        Self {
            style: BedStyle::Plain,
            thickness_m: None,
            coping_m: DEFAULT_COPING_M,
            parapet_m: 0.0,
        }
    }
}

/// One place along the wall: its outer face point (where the terrain meets it), its inner face
/// point (on the outline), the ground there and the wall's bottom and top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Station {
    pub outer: P2,
    pub inner: P2,
    pub ground: f32,
    pub lo: f32,
    pub hi: f32,
}

/// What a terrace costs: the earth moved and the wall built.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub cut_m3: f32,
    pub fill_m3: f32,
    pub wall_length_m: f32,
    /// The wall's area on the terrain's side and on the floor's, at its full height.
    pub outer_face_m2: f32,
    pub inner_face_m2: f32,
    /// The tallest the wall stands (bottom to top), metres.
    pub max_height_m: f32,
    pub beams: Option<(usize, f32)>,
    pub plate_area_m2: Option<f32>,
}

impl Terrace {
    fn bed(&self) -> RaisedBed {
        RaisedBed {
            wall_thickness_m: self
                .thickness_m
                .unwrap_or_else(|| self.style.default_wall_m()),
            style: self.style,
            soil: None,
        }
    }

    /// The wall's thickness as drawn (a steel plate is at least [`bed::MIN_PLATE_M`]).
    pub fn thickness(&self) -> f32 {
        self.bed().wall()
    }

    /// Whether the style takes the thickness and the numbers are usable; the reason if not.
    pub fn check(&self) -> Result<(), String> {
        AreaEdge {
            style: self.style,
            thickness_m: self.thickness_m,
        }
        .check()?;
        for (name, v) in [("coping_m", self.coping_m), ("parapet_m", self.parapet_m)] {
            if !(v.is_finite() && (0.0..=MAX_COPING_M).contains(&v)) {
                return Err(format!("`{name}` must be within 0..={MAX_COPING_M} m"));
            }
        }
        Ok(())
    }

    /// The footprint cut out of the terrain: `outline` moved outwards by the wall's thickness,
    /// which is the wall's outer face.
    pub fn hole(&self, outline: &[P2]) -> Vec<P2> {
        offset_outline(outline, self.thickness())
    }

    /// The wall's bottom and top over ground `ground` for a floor at `floor`.
    pub fn profile(&self, floor: f32, ground: f32) -> (f32, f32) {
        if ground >= floor {
            (floor, ground + self.coping_m)
        } else {
            (ground, floor + self.parapet_m)
        }
    }

    /// The wall sampled along the outline: the outline's edges cut into pieces of about half the
    /// terrain's cell, so the top follows the slope. `ground(x, z)` is the terrain's own height.
    pub fn stations(
        &self,
        outline: &[P2],
        floor: f32,
        ground: &dyn Fn(f32, f32) -> f32,
        cell: f32,
    ) -> Vec<Station> {
        let ring = dense(outline, sample_step(outline, cell));
        let outer = offset_outline(&ring, self.thickness());
        ring.iter()
            .zip(&outer)
            .map(|(&inner, &outer)| {
                let g = ground(outer[0], outer[1]);
                let (lo, hi) = self.profile(floor, g);
                Station {
                    outer,
                    inner,
                    ground: g,
                    lo,
                    hi,
                }
            })
            .collect()
    }

    /// The sleepers of the wall: courses of whole beams over the tallest stretch, each kept where
    /// the wall reaches that high, so the top steps with the slope.
    pub fn beams(
        &self,
        outline: &[P2],
        floor: f32,
        ground: &dyn Fn(f32, f32) -> f32,
        cell: f32,
    ) -> Vec<Beam> {
        let step = sample_step(outline, cell);
        let st = self.stations(outline, floor, ground, cell);
        let lo = st.iter().map(|k| k.lo).fold(f32::MAX, f32::min);
        let hi = st
            .iter()
            .map(|k| k.hi)
            .fold(f32::MIN, f32::max)
            .max(lo + 0.05);
        let outer = offset_outline(outline, self.thickness());
        let courses = (((hi - lo) / bed::BEAM_H).round() as usize).max(1);
        let ch = (hi - lo) / courses as f32;
        let wall_at = |p: P2| self.profile(floor, ground(p[0], p[1]));
        bed::beams(&self.bed(), &outer, lo, hi)
            .into_iter()
            .filter_map(|mut b| {
                let n = (((b.u.1 - b.u.0) / step).ceil() as usize).max(1);
                let (mut blo, mut bhi) = (f32::MAX, f32::MIN);
                for k in 0..=n {
                    let u = b.u.0 + (b.u.1 - b.u.0) * k as f32 / n as f32;
                    let (l, h) = wall_at([b.from[0] + b.dir[0] * u, b.from[1] + b.dir[1] * u]);
                    blo = blo.min(l);
                    bhi = bhi.max(h);
                }
                let mid = (b.y.0 + b.y.1) / 2.0;
                if mid >= bhi || b.y.1 <= blo + 0.02 {
                    return None;
                }
                b.top = mid + ch >= bhi;
                Some(b)
            })
            .collect()
    }

    /// Draws the wall on faces 0 (`wall`) and 1 (`wall_rim`) of the sink.
    pub(crate) fn draw(
        &self,
        s: &mut dyn Sink,
        outline: &[P2],
        floor: f32,
        ground: &dyn Fn(f32, f32) -> f32,
        cell: f32,
    ) {
        use bed::{COPING_H, COPING_OUT, FLANGE_H, FLANGE_IN, FLANGE_OUT, LIP_H, RIM, WALL, quad};
        if self.style == BedStyle::Sleepers {
            let t = self.thickness();
            for b in self.beams(outline, floor, ground, cell) {
                bed::beam(s, &b, t);
            }
            return;
        }
        let st = self.stations(outline, floor, ground, cell);
        let n = st.len();
        let outer: Vec<P2> = st.iter().map(|k| k.outer).collect();
        let inner: Vec<P2> = st.iter().map(|k| k.inner).collect();
        let sign = if shoelace_f32(&outer) > 0.0 {
            1.0
        } else {
            -1.0
        };
        let c = |p: P2, y: f32| [p[0], y, p[1]];
        // The wall's own top: below the coping or the folded edge when there is one.
        let top = |k: &Station| match self.style {
            BedStyle::Masonry => (k.hi - COPING_H).max(k.lo),
            BedStyle::Corten => (k.hi - FLANGE_H).max(k.lo),
            _ => k.hi,
        };
        let down = |k: &Station| k.lo - SINK_M;
        // A vertical band along `ring`, from `y0(i)` to `y1(i)`, facing out (`flip` 1) or in.
        let band = |s: &mut dyn Sink,
                    ring: &[P2],
                    y0: &dyn Fn(usize) -> f32,
                    y1: &dyn Fn(usize) -> f32,
                    face: usize,
                    flip: f32| {
            for i in 0..n {
                let (p, q) = (ring[i], ring[(i + 1) % n]);
                let out = [
                    flip * sign * (q[1] - p[1]),
                    0.0,
                    -flip * sign * (q[0] - p[0]),
                ];
                let j = (i + 1) % n;
                quad(
                    s,
                    face,
                    [c(p, y0(i)), c(q, y0(j)), c(q, y1(j)), c(p, y1(i))],
                    out,
                );
            }
        };
        // A horizontal strip between rings `a` and `b`, facing up or down.
        let strip = |s: &mut dyn Sink,
                     a: &[P2],
                     ya: &dyn Fn(usize) -> f32,
                     b: &[P2],
                     yb: &dyn Fn(usize) -> f32,
                     face: usize,
                     up: f32| {
            for i in 0..n {
                let j = (i + 1) % n;
                quad(
                    s,
                    face,
                    [
                        c(a[i], ya(i)),
                        c(a[j], ya(j)),
                        c(b[j], yb(j)),
                        c(b[i], yb(i)),
                    ],
                    [0.0, up, 0.0],
                );
            }
        };
        let (y_down, y_top) = (|i: usize| down(&st[i]), |i: usize| top(&st[i]));
        let y_hi = |i: usize| st[i].hi;
        band(s, &outer, &y_down, &y_top, WALL, 1.0);
        band(s, &inner, &y_down, &y_top, WALL, -1.0);
        match self.style {
            BedStyle::Plain => strip(s, &outer, &y_hi, &inner, &y_hi, RIM, 1.0),
            BedStyle::Gabion => {
                strip(s, &outer, &y_hi, &inner, &y_hi, RIM, 1.0);
                // Wire bars across the outer face.
                let off: Vec<P2> = (0..n)
                    .map(|i| {
                        let d = [outer[i][0] - inner[i][0], outer[i][1] - inner[i][1]];
                        let l = d[0].hypot(d[1]).max(1e-6);
                        [
                            outer[i][0] + d[0] / l * BAR_OUT,
                            outer[i][1] + d[1] / l * BAR_OUT,
                        ]
                    })
                    .collect();
                let lowest = st.iter().map(|k| k.lo).fold(f32::MAX, f32::min);
                let highest = st.iter().map(|k| k.hi).fold(f32::MIN, f32::max);
                let mut y = (lowest / BAR_PITCH_M).ceil() * BAR_PITCH_M;
                while y < highest {
                    for i in 0..n {
                        let j = (i + 1) % n;
                        let (a, b) = (&st[i], &st[j]);
                        let (lo, hi) = (a.lo.max(b.lo) + 0.03, a.hi.min(b.hi) - BAR_H - 0.03);
                        if y >= lo && y <= hi {
                            let (p, q) = (off[i], off[j]);
                            let out = [sign * (q[1] - p[1]), 0.0, -sign * (q[0] - p[0])];
                            quad(
                                s,
                                RIM,
                                [c(p, y), c(q, y), c(q, y + BAR_H), c(p, y + BAR_H)],
                                out,
                            );
                        }
                    }
                    y += BAR_PITCH_M;
                }
            }
            BedStyle::Masonry => {
                let out = offset_outline(&outer, COPING_OUT);
                // The coping slab: faces out and in, its top and the underside of its overhang.
                band(s, &out, &y_top, &y_hi, RIM, 1.0);
                band(s, &inner, &y_top, &y_hi, RIM, -1.0);
                strip(s, &out, &y_hi, &inner, &y_hi, RIM, 1.0);
                strip(s, &out, &y_top, &outer, &y_top, WALL, -1.0);
            }
            BedStyle::Corten => {
                let out = offset_outline(&outer, FLANGE_OUT);
                let flange_in =
                    crate::shape::inner_ring(&inner, FLANGE_IN).unwrap_or_else(|_| inner.clone());
                // A folded edge over the top and a lip down the outside.
                strip(s, &out, &y_hi, &flange_in, &y_hi, RIM, 1.0);
                strip(s, &out, &y_top, &flange_in, &y_top, RIM, -1.0);
                let lip = |i: usize| (st[i].hi - LIP_H).max(st[i].lo);
                band(s, &out, &lip, &y_hi, RIM, 1.0);
            }
            BedStyle::Sleepers => {}
        }
    }

    /// The earth moved and the wall built; `ground(x, z)` is the terrain's own height.
    pub fn report(
        &self,
        outline: &[P2],
        floor: f32,
        ground: &dyn Fn(f32, f32) -> f32,
        cell: f32,
    ) -> Report {
        let (cut_m3, fill_m3) = volumes(outline, floor, ground);
        let st = self.stations(outline, floor, ground, cell);
        let n = st.len();
        let (mut outer_len, mut inner_len, mut outer_a, mut inner_a) = (0.0, 0.0, 0.0, 0.0);
        let mut max_height_m = 0.0f32;
        for i in 0..n {
            let (a, b) = (&st[i], &st[(i + 1) % n]);
            let h = ((a.hi - a.lo) + (b.hi - b.lo)) / 2.0;
            let lo = (a.outer[0] - b.outer[0]).hypot(a.outer[1] - b.outer[1]);
            let li = (a.inner[0] - b.inner[0]).hypot(a.inner[1] - b.inner[1]);
            outer_len += lo;
            inner_len += li;
            outer_a += lo * h;
            inner_a += li * h;
            max_height_m = max_height_m.max(a.hi - a.lo);
        }
        let wall_length_m = (outer_len + inner_len) / 2.0;
        let beams = (self.style == BedStyle::Sleepers).then(|| {
            let beams = self.beams(outline, floor, ground, cell);
            let length = beams.iter().map(|b| b.u.1 - b.u.0).sum();
            (beams.len(), length)
        });
        let plate_area_m2 = (self.style == BedStyle::Corten).then(|| (outer_a + inner_a) / 2.0);
        Report {
            cut_m3,
            fill_m3,
            wall_length_m,
            outer_face_m2: outer_a,
            inner_face_m2: inner_a,
            max_height_m,
            beams,
            plate_area_m2,
        }
    }
}

impl Report {
    /// The report as the commands and `query.quantities` give it.
    pub fn json(&self, terrace: &Terrace) -> Value {
        let mut v = json!({
            "style": terrace.style,
            "thickness_m": terrace.thickness(),
            "cut_m3": self.cut_m3,
            "fill_m3": self.fill_m3,
            "net_cut_m3": self.cut_m3 - self.fill_m3,
            "wall_length_m": self.wall_length_m,
            "outer_face_area_m2": self.outer_face_m2,
            "inner_face_area_m2": self.inner_face_m2,
            "max_height_m": self.max_height_m,
        });
        if let Some((count, length)) = self.beams {
            v["beams"] = count.into();
            v["beam_length_m"] = length.into();
        }
        if let Some(a) = self.plate_area_m2 {
            v["plate_area_m2"] = a.into();
        }
        v
    }
}

/// How far apart the stations along the wall lie: half the terrain's cell (at least 0.2 m, at
/// most 1 m), wider for a very long outline so the wall stays a few thousand pieces.
fn sample_step(outline: &[P2], cell: f32) -> f32 {
    let n = outline.len();
    let perimeter: f32 = (0..n)
        .map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            (a[0] - b[0]).hypot(a[1] - b[1])
        })
        .sum();
    let cell = if cell.is_finite() && cell > 0.0 {
        cell
    } else {
        DEFAULT_CELL_M
    };
    (cell / 2.0).clamp(0.2, 1.0).max(perimeter / 3000.0)
}

/// `outline` with every edge cut into pieces of at most `step`; the original points stay.
fn dense(outline: &[P2], step: f32) -> Vec<P2> {
    let n = outline.len();
    let mut out = Vec::new();
    for i in 0..n {
        let (a, b) = (outline[i], outline[(i + 1) % n]);
        let len = (a[0] - b[0]).hypot(a[1] - b[1]);
        let k = ((len / step).ceil() as usize).max(1);
        for j in 0..k {
            let f = j as f32 / k as f32;
            out.push([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]);
        }
    }
    out
}

/// The earth to cut and to fill, in m3: the original ground minus the floor over the outline,
/// summed on a fine grid (cell centres, 0.1 m or coarser for a very big outline), the part above
/// the floor as cut and the part below it as fill.
pub fn volumes(outline: &[P2], floor: f32, ground: &dyn Fn(f32, f32) -> f32) -> (f32, f32) {
    let lo = outline
        .iter()
        .fold([f32::MAX; 2], |m, p| [m[0].min(p[0]), m[1].min(p[1])]);
    let hi = outline
        .iter()
        .fold([f32::MIN; 2], |m, p| [m[0].max(p[0]), m[1].max(p[1])]);
    if !(lo[0] < hi[0] && lo[1] < hi[1]) {
        return (0.0, 0.0);
    }
    let area = f64::from(hi[0] - lo[0]) * f64::from(hi[1] - lo[1]);
    let step = (area / 400_000.0).sqrt().max(0.1) as f32;
    let nx = (((hi[0] - lo[0]) / step).ceil() as usize).max(1);
    let nz = (((hi[1] - lo[1]) / step).ceil() as usize).max(1);
    let (sx, sz) = ((hi[0] - lo[0]) / nx as f32, (hi[1] - lo[1]) / nz as f32);
    let cell = f64::from(sx) * f64::from(sz);
    let (mut cut, mut fill) = (0.0f64, 0.0f64);
    for j in 0..nz {
        let z = lo[1] + (j as f32 + 0.5) * sz;
        for i in 0..nx {
            let x = lo[0] + (i as f32 + 0.5) * sx;
            if !inside_outline([x, z], outline) {
                continue;
            }
            let d = f64::from(ground(x, z) - floor);
            if d > 0.0 {
                cut += d * cell;
            } else {
                fill -= d * cell;
            }
        }
    }
    (cut as f32, fill as f32)
}

/// The lowest, highest and mean of the ground under an outline (sampled like [`volumes`]); `None`
/// for an outline with no area.
pub fn ground_stats(outline: &[P2], ground: &dyn Fn(f32, f32) -> f32) -> Option<(f32, f32, f32)> {
    let lo = outline
        .iter()
        .fold([f32::MAX; 2], |m, p| [m[0].min(p[0]), m[1].min(p[1])]);
    let hi = outline
        .iter()
        .fold([f32::MIN; 2], |m, p| [m[0].max(p[0]), m[1].max(p[1])]);
    if !(lo[0] < hi[0] && lo[1] < hi[1]) {
        return None;
    }
    let area = f64::from(hi[0] - lo[0]) * f64::from(hi[1] - lo[1]);
    let step = (area / 40_000.0).sqrt().max(0.1) as f32;
    let nx = (((hi[0] - lo[0]) / step).ceil() as usize).max(1);
    let nz = (((hi[1] - lo[1]) / step).ceil() as usize).max(1);
    let (sx, sz) = ((hi[0] - lo[0]) / nx as f32, (hi[1] - lo[1]) / nz as f32);
    let (mut min, mut max, mut sum, mut count) = (f32::MAX, f32::MIN, 0.0f64, 0usize);
    for j in 0..nz {
        for i in 0..nx {
            let p = [lo[0] + (i as f32 + 0.5) * sx, lo[1] + (j as f32 + 0.5) * sz];
            if inside_outline(p, outline) {
                let g = ground(p[0], p[1]);
                min = min.min(g);
                max = max.max(g);
                sum += f64::from(g);
                count += 1;
            }
        }
    }
    (count > 0).then(|| (min, max, (sum / count as f64) as f32))
}

// --- A terrace in a project ------------------------------------------------------------------

/// The terrain's own height (0 where there is none) and its cell size, for the wall and the
/// earth moved: a terrace is measured against the ground it replaces, not against other terraces.
pub fn terrain_ground(project: &Project) -> (impl Fn(f32, f32) -> f32 + '_, f32) {
    let t = project.terrain.as_deref();
    (
        move |x, z| t.and_then(|t| t.height_at(x, z)).unwrap_or(0.0),
        t.map_or(DEFAULT_CELL_M, |t| t.cell_m),
    )
}

/// The report of terrace area `a` in `project`; `None` for an area without a terrace.
pub fn report_for(project: &Project, a: &crate::shape::Area) -> Option<Report> {
    let t = a.terrace.as_ref()?;
    let (ground, cell) = terrain_ground(project);
    Some(t.report(&a.outline(), a.elevation, &ground, cell))
}

/// What to tell the person about terrace area `a`: a wall that needs an engineer, no terrain
/// under it, a floor that is above the ground everywhere.
pub fn warnings(project: &Project, a: &crate::shape::Area, r: &Report) -> Vec<String> {
    let mut out = Vec::new();
    if r.max_height_m > ENGINEER_M {
        out.push(format!(
            "the retaining wall stands up to {:.2} m high; a wall over {ENGINEER_M} m needs an engineer's design",
            r.max_height_m
        ));
    }
    let outline = a.outline();
    let missed = std::cell::Cell::new(0usize);
    let seen = std::cell::Cell::new(0usize);
    if let Some(t) = project.terrain.as_deref() {
        ground_stats(&outline, &|x, z| {
            seen.set(seen.get() + 1);
            let h = t.height_at(x, z);
            if h.is_none() {
                missed.set(missed.get() + 1);
            }
            h.unwrap_or(0.0)
        });
    }
    if project.terrain.is_none() || (seen.get() > 0 && missed.get() == seen.get()) {
        out.push(
            "there is no terrain under this terrace, so the ground is taken as flat at 0; make a terrain first (terrain.create)"
                .into(),
        );
    } else if missed.get() > 0 {
        out.push(
            "part of this terrace lies off the terrain: the ground there is taken as 0".into(),
        );
    }
    if r.cut_m3 < 0.005 && r.fill_m3 > 0.0 {
        out.push(
            "the floor lies above the ground everywhere, so nothing is cut: this is a raised area, not a cut terrace; use area.set_edge for a retaining edge instead"
                .into(),
        );
    }
    out
}

// --- Ground queries -------------------------------------------------------------------------

/// A terrace as the ground queries see it.
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    /// The terrace's area.
    pub id: String,
    /// The floor's outline.
    pub outline: Vec<P2>,
    /// The footprint cut out of the terrain: the outline moved out by the wall.
    pub hole: Vec<P2>,
    /// The floor's height.
    pub floor: f32,
    lo: P2,
    hi: P2,
}

impl Zone {
    fn boxed(&self, p: P2) -> bool {
        p[0] >= self.lo[0] && p[0] <= self.hi[0] && p[1] >= self.lo[1] && p[1] <= self.hi[1]
    }

    /// The floor's height when `p` is inside the outline.
    pub fn floor_at(&self, p: P2) -> Option<f32> {
        (self.boxed(p) && inside_outline(p, &self.outline)).then_some(self.floor)
    }

    /// Whether `p` lies where the terrain is cut out (the floor and its wall).
    pub fn in_hole(&self, p: P2) -> bool {
        self.boxed(p) && inside_outline(p, &self.hole)
    }
}

/// The terraces of the shapes, in shape order (a later one is on top where they overlap).
pub fn zones(shapes: &[Shape]) -> Vec<Zone> {
    shapes
        .iter()
        .filter_map(|s| match s {
            Shape::Area(a) => {
                let t = a.terrace.as_ref()?;
                let outline = a.outline().into_owned();
                let hole = t.hole(&outline);
                let lo = hole
                    .iter()
                    .fold([f32::MAX; 2], |m, p| [m[0].min(p[0]), m[1].min(p[1])]);
                let hi = hole
                    .iter()
                    .fold([f32::MIN; 2], |m, p| [m[0].max(p[0]), m[1].max(p[1])]);
                Some(Zone {
                    id: a.id.clone(),
                    outline,
                    hole,
                    floor: a.elevation,
                    lo,
                    hi,
                })
            }
            Shape::Extrusion(_) => None,
        })
        .collect()
}

/// The footprints cut out of the terrain's meshes.
pub fn holes(zones: &[Zone]) -> Vec<Vec<P2>> {
    zones.iter().map(|z| z.hole.clone()).collect()
}

/// The height of the ground at `(x, z)`: a terrace's floor inside its outline, else the terrain's
/// (`None` where there is none). This is the one function placement asks.
pub fn ground_at(terrain: Option<&Terrain>, zones: &[Zone], x: f32, z: f32) -> Option<f32> {
    zones
        .iter()
        .rev()
        .find_map(|zone| zone.floor_at([x, z]))
        .or_else(|| terrain.and_then(|t| t.height_at(x, z)))
}

/// [`ground_at`] for a project, as a function (the terraces found once).
pub fn ground_fn(project: &Project) -> impl Fn(f32, f32) -> Option<f32> + '_ {
    let zones = zones(&project.shapes);
    let terrain = project.terrain.as_deref();
    move |x, z| ground_at(terrain, &zones, x, z)
}

/// The first hit of a ray with the terrain as it is drawn: where a terrace cuts it out, the ray
/// goes on through to whatever is below.
pub fn terrain_raycast(
    terrain: &Terrain,
    zones: &[Zone],
    origin: [f32; 3],
    dir: [f32; 3],
) -> Option<(f32, [f32; 3], [f32; 3])> {
    if zones.is_empty() {
        return terrain.raycast(origin, dir);
    }
    terrain.raycast_where(origin, dir, &|p| {
        !zones.iter().any(|z| z.in_hole([p[0], p[2]]))
    })
}

/// [`terrain_raycast`] for a project: `None` without a terrain.
pub fn project_raycast(
    project: &Project,
    origin: [f32; 3],
    dir: [f32; 3],
) -> Option<(f32, [f32; 3], [f32; 3])> {
    let t = project.terrain.as_deref()?;
    terrain_raycast(t, &zones(&project.shapes), origin, dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::{Area, shape_mesh_on_terrain_with};

    pub(crate) fn rect(x0: f32, z0: f32, x1: f32, z1: f32) -> Vec<P2> {
        vec![[x0, z0], [x1, z0], [x1, z1], [x0, z1]]
    }

    /// A 40 x 30 m terrain (0.5 m cells) sloping down towards +x: the ground is `1 - 0.2 x`.
    pub(crate) fn slope() -> Terrain {
        let mut t = Terrain::flat([0.0, 0.0], 40.0, 30.0, 0.5, "lawn").unwrap();
        let w = t.width as usize + 1;
        for (i, h) in t.heights.iter_mut().enumerate() {
            *h = 1.0 - 0.2 * (i % w) as f32 * 0.5;
        }
        t
    }

    pub(crate) fn area(outline: Vec<P2>, floor: f32, terrace: Terrace) -> Shape {
        Shape::Area(Area {
            id: "S1".into(),
            name: "Terrace".into(),
            layer: "shapes".into(),
            points: outline,
            curves: Default::default(),
            smooth: false,
            elevation: floor,
            drape: false,
            edge_blend_m: 0.0,
            edge: None,
            terrace: Some(terrace),
            extensions: Default::default(),
        })
    }

    fn plane(x: f32, _z: f32) -> f32 {
        1.0 - 0.2 * x
    }

    #[test]
    fn cut_and_fill_match_the_analytic_volume_on_a_slope() {
        // 10 x 6 m between x = 5 and 15: the ground is 0 at 5 and -2 at 15. A floor at -1 cuts
        // the part above it (x < 10, 0..1 high, a wedge) and fills the rest.
        let g = |x: f32, z: f32| plane(x, z);
        let (cut, fill) = volumes(&rect(5.0, 2.0, 15.0, 8.0), -1.0, &g);
        // Above -1 for x < 10: wedge of depth 1 over 5 m: 0.5 * 5 * 1 * 6 = 15 m3.
        // Below -1 for x > 10: wedge of depth 1 over 5 m: 15 m3.
        assert!((cut - 15.0).abs() < 0.3, "{cut}");
        assert!((fill - 15.0).abs() < 0.3, "{fill}");
        // A floor under all the ground only cuts: the mean ground is -1, so 1 m3 per m2 more.
        let (cut, fill) = volumes(&rect(5.0, 2.0, 15.0, 8.0), -3.0, &g);
        assert!((cut - 120.0).abs() < 2.4 && fill == 0.0, "{cut} {fill}");
    }

    #[test]
    fn the_wall_top_and_bottom_follow_the_ground_on_a_slope() {
        let t = Terrace {
            parapet_m: 0.45,
            ..Terrace::default()
        };
        // Floor at -1 between x = 5 and 15 on the slope that is 0 at 5 and -2 at 15.
        let st = t.stations(&rect(5.0, 2.0, 15.0, 8.0), -1.0, &plane, 1.0);
        assert!(st.len() > 40);
        let thick = t.thickness();
        for k in &st {
            assert!((k.ground - plane(k.outer[0], k.outer[1])).abs() < 1e-5);
            if k.ground >= -1.0 {
                assert_eq!((k.lo, k.hi), (-1.0, k.ground + t.coping_m), "{k:?}");
            } else {
                assert_eq!((k.lo, k.hi), (k.ground, -1.0 + 0.45), "{k:?}");
            }
            // The outer face is the wall's thickness out from the inner one.
            let d = (k.outer[0] - k.inner[0]).hypot(k.outer[1] - k.inner[1]);
            assert!(d >= thick - 1e-4 && d <= thick * 1.5 + 1e-4, "{d}");
        }
        // The uphill (x = 5) wall is a cut: its top is the ground plus the coping; downhill it is
        // the floor plus the parapet, standing 1 m off the ground.
        let uphill = st
            .iter()
            .find(|k| k.inner[0] < 5.01 && k.inner[1] == 5.0)
            .unwrap();
        assert!((uphill.hi - (plane(5.0 - thick, 5.0) + 0.1)).abs() < 0.01);
        let downhill = st
            .iter()
            .find(|k| k.inner[0] > 14.99 && k.inner[1] == 5.0)
            .unwrap();
        assert!((downhill.hi - downhill.lo - (0.45 + (-1.0 - downhill.ground))).abs() < 1e-4);
        // The report: a wall of 32 m round, taller downhill than at the cut's end.
        let r = t.report(&rect(5.0, 2.0, 15.0, 8.0), -1.0, &plane, 1.0);
        assert!((r.wall_length_m - 32.0).abs() < 1.0, "{}", r.wall_length_m);
        assert!(r.max_height_m > 1.2, "{}", r.max_height_m);
        assert!(r.outer_face_m2 > 10.0 && r.inner_face_m2 > 10.0);
    }

    #[test]
    fn the_ground_is_the_floor_inside_a_terrace_and_the_terrain_outside() {
        let terrain = slope();
        let shapes = [area(rect(5.0, 2.0, 15.0, 8.0), -1.0, Terrace::default())];
        let z = zones(&shapes);
        assert_eq!(z.len(), 1);
        assert_eq!(ground_at(Some(&terrain), &z, 10.0, 5.0), Some(-1.0));
        assert_eq!(
            ground_at(Some(&terrain), &z, 20.0, 5.0),
            terrain.height_at(20.0, 5.0)
        );
        assert_eq!(
            ground_at(Some(&terrain), &[], 10.0, 5.0),
            terrain.height_at(10.0, 5.0)
        );
        // Off the terrain the floor still answers inside the terrace, nothing outside.
        assert_eq!(ground_at(None, &z, 10.0, 5.0), Some(-1.0));
        assert_eq!(ground_at(None, &z, 30.0, 5.0), None);
        // A ray from above passes through the ghost of the cut ground inside the hole.
        let up = terrain_raycast(&terrain, &z, [10.0, 20.0, 5.0], [0.0, -1.0, 0.0]);
        assert!(up.is_none(), "{up:?}");
        let out = terrain_raycast(&terrain, &z, [20.0, 20.0, 5.0], [0.0, -1.0, 0.0]).unwrap();
        assert!((out.1[1] - terrain.height_at(20.0, 5.0).unwrap()).abs() < 1e-3);
    }

    fn extent(m: &crate::ImportedModel) -> ([f32; 3], [f32; 3]) {
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
    fn every_style_draws_a_wall_with_its_faces_that_stays_in_its_band() {
        let terrain = slope();
        for style in [
            BedStyle::Plain,
            BedStyle::Masonry,
            BedStyle::Sleepers,
            BedStyle::Corten,
            BedStyle::Gabion,
        ] {
            let t = Terrace {
                style,
                parapet_m: 0.3,
                ..Terrace::default()
            };
            let shape = area(rect(5.0, 2.0, 15.0, 8.0), -1.0, t);
            let m = shape_mesh_on_terrain_with(&shape, &terrain, &[], &[]).unwrap();
            let names: Vec<&str> = m.faces.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(names, ["S1:top", "S1:wall", "S1:wall_rim"], "{style:?}");
            assert!(
                m.triangle_faces.contains(&1) && m.triangle_faces.contains(&2),
                "{style:?}"
            );
            let (lo, hi) = extent(&m);
            let th = t.thickness();
            // Inside the hole (a little over it for a coping or a fold), and as high as the
            // wall's top somewhere.
            assert!(
                lo[0] > 5.0 - th - 0.1 && hi[0] < 15.0 + th + 0.1,
                "{style:?} {lo:?} {hi:?}"
            );
            assert!(hi[1] > 0.0 && hi[1] < 1.0, "{style:?} {hi:?}");
        }
    }

    #[test]
    fn a_terrace_refuses_what_it_cannot_be() {
        let terrain = slope();
        let base = area(rect(5.0, 2.0, 15.0, 8.0), -1.0, Terrace::default());
        let with = |f: &dyn Fn(&mut Area)| {
            let mut shape = base.clone();
            if let Shape::Area(a) = &mut shape {
                f(a);
            }
            shape
        };
        let draped = with(&|a| a.drape = true);
        assert!(shape_mesh_on_terrain_with(&draped, &terrain, &[], &[]).is_err());
        let edged = with(&|a| {
            a.edge = Some(AreaEdge {
                style: BedStyle::Plain,
                thickness_m: None,
            });
        });
        assert!(crate::shape::shape_mesh(&edged).is_err());
        let thick = with(&|a| {
            a.terrace = Some(Terrace {
                thickness_m: Some(5.0),
                ..Terrace::default()
            });
        });
        assert!(crate::shape::shape_mesh(&thick).is_err());
        let nan = with(&|a| {
            a.terrace = Some(Terrace {
                parapet_m: f32::NAN,
                ..Terrace::default()
            });
        });
        assert!(crate::shape::shape_mesh(&nan).is_err());
        assert!(crate::shape::shape_mesh(&base).is_ok());
    }
}
