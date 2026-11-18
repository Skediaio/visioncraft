//! Garden stairs set into the slope they climb (`Stairs::set_in`).
//!
//! A flight with a [`SetIn`] follows the ground instead of standing on it:
//!
//! - **the goings follow the ground** ([`fit_goings`]): the step count is still the rise over the
//!   riser, but each riser stands where the original ground (terrain and terraces, along the
//!   centreline) reaches the level of the step before, so a tread lies within one riser of the
//!   ground over its whole depth. A going is held within [`MIN_GOING_M`]..[`MAX_GOING_M`]: a flat
//!   stretch becomes a long tread, a steep one is warned about. A flight that turns does this per
//!   flight; the landings are as they were;
//! - **the footprint is cut out of the terrain** like a terrace's ([`Built::hole`]): the outline,
//!   widened by the cheeks' thickness along the sides (not at the bottom and the top), is clipped
//!   out of the terrain's chunk meshes, and a ground question inside the flight answers with the
//!   tread's level ([`zones`], `crate::terrace::Zone`), so nothing grows on the steps;
//! - **cheek walls** stand along both sides ([`Built::draw`]): from below the step up to the
//!   ground beside it plus `coping_m` where they hold earth, and just under the tread where the
//!   ground is lower. The wall follows the ground in pieces of about half the terrain's cell, and
//!   steps with the treads. It has a [`BedStyle`], drawn by the same builders a terrace's wall is:
//!   plain, masonry, sleepers, corten or gabion. Faces `<id>:cheeks` and `<id>:cheeks_rim`. With
//!   style `none` there are no cheeks: the terrain is still cut, and a bank is not supported.
//!
//! Everything here is in the flight's own frame, relative to `lift` (the lowest ground under the
//! footprint, where the shape is placed) like the rest of a flight's mesh.

use crate::bed::{
    self, AreaEdge, Beam, BedStyle, COPING_H, COPING_OUT, FLANGE_H, FLANGE_IN, FLANGE_OUT, LIP_H,
    quad,
};
use crate::house::Sink;
use crate::project::Project;
use crate::shape::{Extrusion, Shape, footing, shoelace_f32};
use crate::stairs::{Block, Geometry};
use crate::terrace::{ENGINEER_M, Tread, Zone};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

type P2 = [f32; 2];

/// Faces the cheeks have after a flight's own three, in this order.
pub const FACES: [&str; 2] = ["cheeks", "cheeks_rim"];
const WALL: usize = 0;
const RIM: usize = 1;

/// The shallowest and the deepest going a flight set into the slope has (metres).
pub const MIN_GOING_M: f32 = 0.25;
pub const MAX_GOING_M: f32 = 2.0;
/// The goings that are comfortable by themselves; outside them Blondel's rule must hold.
pub const COMFORT_GOING_M: (f32, f32) = (0.28, 0.45);
/// How far a cheek stands above the ground it holds back when nothing else is asked (metres).
pub const DEFAULT_COPING_M: f32 = 0.1;
/// The tallest coping (metres).
pub const MAX_COPING_M: f32 = 1.0;
/// How far a cheek is sunk under the lowest of its ground, the tread and the step's block, so
/// the terrain that meets it at a coarser level of detail leaves no gap (metres).
const SINK_M: f32 = 0.05;
/// How far a cheek's top stands under a tread where the ground beside it is lower (metres).
const BELOW_TREAD_M: f32 = 0.02;
/// The longest sleeper in a cheek: shorter than a bed's, so the top steps with the treads.
const BEAM_LEN_M: f32 = 1.2;
const BAR_PITCH_M: f32 = 0.15;
const BAR_H: f32 = 0.012;
const BAR_OUT: f32 = 0.008;

/// The cheek walls' style: a bed's, or none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheekStyle {
    None,
    #[default]
    Plain,
    Masonry,
    Sleepers,
    Corten,
    Gabion,
}

impl CheekStyle {
    /// The wall style, `None` for no cheeks.
    pub fn bed(self) -> Option<BedStyle> {
        match self {
            Self::None => None,
            Self::Plain => Some(BedStyle::Plain),
            Self::Masonry => Some(BedStyle::Masonry),
            Self::Sleepers => Some(BedStyle::Sleepers),
            Self::Corten => Some(BedStyle::Corten),
            Self::Gabion => Some(BedStyle::Gabion),
        }
    }
}

fn default_coping() -> f32 {
    DEFAULT_COPING_M
}

fn is_default_coping(v: &f32) -> bool {
    *v == DEFAULT_COPING_M
}

/// How a flight is set into the slope.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetIn {
    /// The cheek walls along both sides, or `none` for a flight with the terrain cut and no walls.
    #[serde(default)]
    pub cheek_style: CheekStyle,
    /// How thick the cheeks are (a sleeper's thickness for sleepers, the plate's for corten); the
    /// style's own when absent. They stand outside the flight, so it keeps its width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thickness_m: Option<f32>,
    /// How far a cheek's top stands above the ground where it holds earth.
    #[serde(default = "default_coping", skip_serializing_if = "is_default_coping")]
    pub coping_m: f32,
}

impl Default for SetIn {
    fn default() -> Self {
        Self {
            cheek_style: CheekStyle::default(),
            thickness_m: None,
            coping_m: DEFAULT_COPING_M,
        }
    }
}

impl SetIn {
    fn edge(&self) -> Option<AreaEdge> {
        self.cheek_style.bed().map(|style| AreaEdge {
            style,
            thickness_m: self.thickness_m,
        })
    }

    /// The cheeks' thickness as drawn; 0 without cheeks.
    pub fn thickness(&self) -> f32 {
        self.edge().map_or(0.0, |e| e.wall())
    }

    /// Whether the numbers are usable; the reason if not.
    pub fn check(&self) -> Result<(), String> {
        if let Some(e) = self.edge() {
            e.check()?;
        }
        if !(self.coping_m.is_finite() && (0.0..=MAX_COPING_M).contains(&self.coping_m)) {
            return Err(format!("`coping_m` must be within 0..={MAX_COPING_M} m"));
        }
        Ok(())
    }
}

// --- The goings -----------------------------------------------------------------------------

/// The goings of a flight and what to tell about them.
pub(crate) struct Fit {
    pub goings: Vec<f32>,
    pub notes: Vec<String>,
}

/// The steps' goings along a run of `run` m: `ground(s)` is the ground's height above the bottom
/// of the whole flight at distance `s` from the flight's start, `level0` the height of the
/// flight's bottom step's foot (the level the first riser stands on), `riser` the rise of a step.
/// Riser `k` stands where the ground first reaches the level of the step before it, so tread `k`
/// lies between the ground at the height of its foot and of its top. Each going but the last is
/// held within [`MIN_GOING_M`]..[`MAX_GOING_M`]; the last runs on to the end of the run.
pub(crate) fn fit_goings(
    ground: &dyn Fn(f32) -> f32,
    run: f32,
    n: usize,
    riser: f32,
    level0: f32,
    label: &str,
) -> Result<Fit, String> {
    if n as f32 * MIN_GOING_M > run + 1e-4 {
        return Err(format!(
            "{label}the flight is too short for {n} steps: each needs a going of at least {MIN_GOING_M} m, and it is {run:.2} m long"
        ));
    }
    let count = ((run / 0.05).ceil() as usize).clamp(2, 4000);
    let ds = run / count as f32;
    let gs: Vec<f32> = (0..=count).map(|i| ground(ds * i as f32)).collect();
    let at = |s: f32| {
        let i = ((s / ds).floor() as usize).min(count - 1);
        let f = (s - ds * i as f32) / ds;
        gs[i] + (gs[i + 1] - gs[i]) * f
    };
    // The first distance from `from` on where the ground has reached `level`.
    let reach = |from: f32, level: f32| -> f32 {
        if at(from) >= level {
            return from;
        }
        let first = ((from / ds).floor() as usize).min(count - 1) + 1;
        for i in first..=count {
            if gs[i] >= level {
                let (a, b) = (gs[i - 1], gs[i]);
                let s =
                    ds * (i - 1) as f32 + ds * ((level - a) / (b - a).max(1e-9)).clamp(0.0, 1.0);
                return s.max(from);
            }
        }
        run
    };
    let (mut pos, mut goings) = (0.0f32, Vec::with_capacity(n));
    let (mut short, mut long) = (Vec::new(), Vec::new());
    for k in 1..n {
        let raw = reach(pos, level0 + k as f32 * riser) - pos;
        if raw < MIN_GOING_M - 1e-4 {
            short.push(k);
        } else if raw > MAX_GOING_M + 1e-4 {
            long.push(k);
        }
        let g = raw.clamp(MIN_GOING_M, MAX_GOING_M);
        goings.push(g);
        pos += g;
    }
    let mut notes = Vec::new();
    // The last step keeps at least the minimum going: take the excess off the others.
    let room = run - MIN_GOING_M;
    let mut squeezed = false;
    if pos > room + 1e-4 {
        let excess = pos - room;
        let spare: f32 = goings.iter().map(|g| g - MIN_GOING_M).sum();
        if spare > 1e-6 {
            for g in &mut goings {
                *g -= (*g - MIN_GOING_M) * (excess / spare).min(1.0);
            }
            squeezed = true;
        }
    }
    let last = run - goings.iter().sum::<f32>();
    goings.push(last);
    let list = |v: &[usize]| {
        v.iter()
            .map(|k| k.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !short.is_empty() {
        notes.push(format!(
            "{label}steps {}: the ground is steeper than the steps can follow, so the going is held at {MIN_GOING_M} m and the treads there stand proud of the slope",
            list(&short)
        ));
    }
    if !long.is_empty() {
        notes.push(format!(
            "{label}steps {}: the ground is flatter than a step needs, so the going is held at {MAX_GOING_M} m",
            list(&long)
        ));
    }
    if squeezed {
        notes.push(format!(
            "{label}the flight is too short for the slope it climbs: the goings were shortened to fit its {run:.2} m"
        ));
    }
    // A top step that runs on to a landing is not a going.
    let rated = if last > COMFORT_GOING_M.1 { n - 1 } else { n };
    let blondel = |g: f32| 2.0 * riser + g;
    let odd: Vec<usize> = (0..rated)
        .filter(|&k| {
            let g = goings[k];
            !(COMFORT_GOING_M.0 - 1e-3..=COMFORT_GOING_M.1 + 1e-3).contains(&g)
                && !(crate::stairs::BLONDEL_M.0 - 1e-3..=crate::stairs::BLONDEL_M.1 + 1e-3)
                    .contains(&blondel(g))
        })
        .map(|k| k + 1)
        .collect();
    if !odd.is_empty() {
        let (lo, hi) = odd.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &k| {
            (lo.min(goings[k - 1]), hi.max(goings[k - 1]))
        });
        notes.push(format!(
            "{label}uncomfortable steps {}: goings {lo:.2}..{hi:.2} m are outside {:.2}..{:.2} m and 2 x riser {riser:.3} + going is outside Blondel's {:.2}..{:.2} m",
            list(&odd),
            COMFORT_GOING_M.0,
            COMFORT_GOING_M.1,
            crate::stairs::BLONDEL_M.0,
            crate::stairs::BLONDEL_M.1
        ));
    }
    Ok(Fit { goings, notes })
}

// --- The footprint and the cheeks ---------------------------------------------------------

/// One cheek wall along an outline edge: the edge's start, its unit direction and outward
/// normal, and the stretch of it the wall covers (`u`, from the start).
#[derive(Clone, Copy, Debug)]
pub struct Run {
    pub from: P2,
    pub dir: P2,
    pub out: P2,
    pub len: f32,
    pub u: (f32, f32),
}

/// One place along a cheek: where its outer face stands, the ground there, and the wall's bottom
/// and top. Heights are relative to `lift`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Station {
    pub outer: P2,
    pub inner: P2,
    pub ground: f32,
    pub lo: f32,
    pub hi: f32,
}

/// A stretch of a cheek along one tread: the tread's level and the stations on it.
#[derive(Clone, Debug)]
pub struct Piece {
    pub tread: f32,
    pub stations: Vec<Station>,
}

/// What a flight set into the slope is: its steps, the earth under it, the cut and its cheeks.
/// All heights are relative to `lift`, the lowest ground under the footprint.
pub struct Built {
    pub geometry: Geometry,
    /// Where the shape stands (absolute height).
    pub lift: f32,
    /// The bottom of each step's block ([`crate::stairs::step_bottoms`]).
    pub bottoms: Vec<f32>,
    pub set_in: SetIn,
    /// The terrain's cut-out: the outline, widened at the sides by the cheeks' thickness.
    pub hole: Vec<P2>,
    pub runs: Vec<Run>,
    /// Per run, the pieces of its cheek.
    pub cheeks: Vec<Vec<Piece>>,
    pub cut_m3: f32,
    pub fill_m3: f32,
    /// How far the ground beside the steps rises above a tread (the bank a flight with no cheeks
    /// leaves open).
    pub bank_m: f32,
    base: f32,
}

/// What a flight set into the slope costs.
pub struct Report {
    pub goings: Vec<f32>,
    pub cut_m3: f32,
    pub fill_m3: f32,
    pub cheek_length_m: f32,
    pub outer_face_m2: f32,
    pub inner_face_m2: f32,
    pub max_height_m: f32,
    pub beams: Option<(usize, f32)>,
    pub plate_area_m2: Option<f32>,
}

/// The ends of a flight: the middle of its bottom edge and of its top edge.
fn ends(blocks: &[Block]) -> (P2, P2) {
    let (a, b) = (&blocks[0], &blocks[blocks.len() - 1]);
    (
        [
            a.centre[0] - a.axis[0] * a.half[0],
            a.centre[1] - a.axis[1] * a.half[0],
        ],
        [
            b.centre[0] + b.axis[0] * b.half[0],
            b.centre[1] + b.axis[1] * b.half[0],
        ],
    )
}

/// The outline edges the cheeks stand along, with the way each is covered: every edge but the
/// flight's bottom and top ones. At a corner that turns outwards one wall runs on by `t` over the
/// corner; at one that turns inwards the next starts `t` in.
fn runs(outline: &[P2], ends: (P2, P2), t: f32) -> Vec<Option<Run>> {
    let n = outline.len();
    let sign = if shoelace_f32(outline) > 0.0 {
        1.0
    } else {
        -1.0
    };
    let edge = |i: usize| {
        let (p, q) = (outline[i], outline[(i + 1) % n]);
        let len = (q[0] - p[0]).hypot(q[1] - p[1]).max(1e-6);
        let d = [(q[0] - p[0]) / len, (q[1] - p[1]) / len];
        (p, q, len, d)
    };
    let near = |p: P2, q: P2| (p[0] - q[0]).hypot(p[1] - q[1]) < 0.02;
    let mut out: Vec<Option<Run>> = (0..n)
        .map(|i| {
            let (p, q, len, d) = edge(i);
            let mid = [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0];
            (!near(mid, ends.0) && !near(mid, ends.1)).then_some(Run {
                from: p,
                dir: d,
                out: [sign * d[1], -sign * d[0]],
                len,
                u: (0.0, len),
            })
        })
        .collect();
    for i in 0..n {
        let j = (i + 1) % n;
        if out[i].is_none() || out[j].is_none() {
            continue;
        }
        let (di, dj) = (edge(i).3, edge(j).3);
        let cross = di[0] * dj[1] - di[1] * dj[0];
        if sign * cross > 0.0 {
            if let Some(r) = &mut out[i] {
                r.u.1 = r.len + t;
            }
        } else if let Some(r) = &mut out[j] {
            r.u.0 = t;
        }
    }
    out
}

/// `outline` with each edge moved out by its own `offsets` (the corners stay on both lines).
fn offset_edges(outline: &[P2], offsets: &[f32]) -> Vec<P2> {
    let n = outline.len();
    let sign = if shoelace_f32(outline) > 0.0 {
        1.0
    } else {
        -1.0
    };
    let out = |i: usize| {
        let (p, q) = (outline[i], outline[(i + 1) % n]);
        let len = (q[0] - p[0]).hypot(q[1] - p[1]).max(1e-6);
        [sign * (q[1] - p[1]) / len, -sign * (q[0] - p[0]) / len]
    };
    (0..n)
        .map(|i| {
            let (a, b) = (out((i + n - 1) % n), out(i));
            let (oa, ob) = (offsets[(i + n - 1) % n], offsets[i]);
            [
                outline[i][0] + a[0] * oa + b[0] * ob,
                outline[i][1] + a[1] * oa + b[1] * ob,
            ]
        })
        .collect()
}

/// Which block of `blocks` holds `p`, or the nearest one.
fn block_at(blocks: &[Block], p: P2) -> usize {
    let mut best = (f32::MAX, 0);
    for (i, b) in blocks.iter().enumerate() {
        let (a, n) = (b.axis, [-b.axis[1], b.axis[0]]);
        let (dx, dz) = (p[0] - b.centre[0], p[1] - b.centre[1]);
        let (u, w) = (dx * a[0] + dz * a[1], dx * n[0] + dz * n[1]);
        let d = (u.abs() - b.half[0]).max(w.abs() - b.half[1]);
        if d < best.0 - 1e-6 {
            best = (d, i);
        }
    }
    best.1
}

/// The cheeks' pieces and stations along `run`.
#[allow(clippy::too_many_arguments)]
fn pieces(
    run: &Run,
    t: f32,
    blocks: &[Block],
    base: f32,
    bottoms: &[f32],
    coping: f32,
    ground: &dyn Fn(f32, f32) -> f32,
    cell: f32,
) -> Vec<Piece> {
    let step = (cell / 2.0).clamp(0.1, 0.5);
    let span = run.u.1 - run.u.0;
    let n = ((span / step).ceil() as usize).clamp(1, 2000);
    let us: Vec<f32> = (0..=n)
        .map(|i| run.u.0 + span * i as f32 / n as f32)
        .collect();
    let inner = |u: f32| [run.from[0] + run.dir[0] * u, run.from[1] + run.dir[1] * u];
    // The block a place beside the flight belongs to: the step the cheek stands against.
    let block = |u: f32| {
        let p = inner(u.clamp(0.0, run.len));
        block_at(blocks, [p[0] - run.out[0] * 0.01, p[1] - run.out[1] * 0.01])
    };
    let station = |u: f32, b: usize| {
        let inner = inner(u);
        let outer = [inner[0] + run.out[0] * t, inner[1] + run.out[1] * t];
        let g = ground(outer[0], outer[1]);
        let tread = base + blocks[b].rise.1;
        let foot = bottoms.get(b).copied().unwrap_or(base);
        let lo = foot.min(g).min(tread) - SINK_M;
        let hi = if g >= tread {
            g + coping
        } else {
            (g + coping).min(tread - BELOW_TREAD_M)
        };
        Station {
            outer,
            inner,
            ground: g,
            lo,
            hi: hi.max(lo + 0.02),
        }
    };
    let level = |b: usize| base + blocks[b].rise.1;
    let mut out: Vec<Piece> = Vec::new();
    let mut cur_block = block(us[0]);
    let mut cur = vec![station(us[0], cur_block)];
    for i in 1..=n {
        let b = block(us[i]);
        if b == cur_block || (level(b) - level(cur_block)).abs() < 1e-5 {
            cur.push(station(us[i], cur_block));
            continue;
        }
        // The tread changes between two samples: find where.
        let (mut a, mut z) = (us[i - 1], us[i]);
        for _ in 0..24 {
            let m = (a + z) / 2.0;
            if (level(block(m)) - level(cur_block)).abs() < 1e-5 {
                a = m;
            } else {
                z = m;
            }
        }
        let edge = (a + z) / 2.0;
        cur.push(station(edge, cur_block));
        out.push(Piece {
            tread: level(cur_block),
            stations: std::mem::take(&mut cur),
        });
        cur_block = b;
        cur = vec![station(edge, cur_block), station(us[i], cur_block)];
    }
    out.push(Piece {
        tread: level(cur_block),
        stations: cur,
    });
    out
}

/// A flight `e` set into the ground, which is `ground(x, z)` in absolute height (terrain and
/// terraces, without the flight itself; `None` where there is none), on a terrain of `cell` m
/// cells; the reason if the flight cannot be drawn.
pub fn build(
    e: &Extrusion,
    ground: &dyn Fn(f32, f32) -> Option<f32>,
    cell: f32,
) -> Result<Built, String> {
    let st = e.stairs.as_ref().ok_or("not stairs")?;
    let set_in = st.set_in.ok_or("not set into the slope")?;
    set_in.check()?;
    let outline = e.outline();
    let lift = footing(&outline, ground);
    let abs = |x: f32, z: f32| ground(x, z).unwrap_or(0.0);
    let rel_base = |x: f32, z: f32| abs(x, z) - lift - e.base;
    let g = st.geometry_with(&e.points, e.height, Some(&rel_base))?;
    let blocks = g.blocks();
    // Each block goes down to the lowest ground under it, but not above its own riser's foot: a
    // flight set into the slope has no use for the blocks reaching down to the bottom step.
    let bottoms: Vec<f32> = crate::stairs::step_bottoms(&g, f32::MAX, lift, ground)
        .into_iter()
        .zip(&blocks)
        .map(|(low, b)| low.min(e.base + b.rise.0))
        .collect();
    let t = set_in.thickness();
    let rel = |x: f32, z: f32| abs(x, z) - lift;
    let (runs, hole, cheeks) = if t > 0.0 {
        let all = runs(&e.points, ends(&blocks), t);
        let offsets: Vec<f32> = all
            .iter()
            .map(|r| if r.is_some() { t } else { 0.0 })
            .collect();
        let hole = offset_edges(&e.points, &offsets);
        let runs: Vec<Run> = all.into_iter().flatten().collect();
        let cheeks = runs
            .iter()
            .map(|r| pieces(r, t, &blocks, e.base, &bottoms, set_in.coping_m, &rel, cell))
            .collect();
        (runs, hole, cheeks)
    } else {
        (Vec::new(), e.points.clone(), Vec::new())
    };
    // The earth under the treads, and the bank beside them.
    let (mut cut, mut fill, mut bank) = (0.0f64, 0.0f64, 0.0f32);
    for b in &blocks {
        let (a, n) = (b.axis, [-b.axis[1], b.axis[0]]);
        let area = 4.0 * b.half[0] * b.half[1];
        let ds = (area / 20_000.0).sqrt().max(0.05);
        let nu = ((2.0 * b.half[0] / ds).ceil() as usize).max(1);
        let nw = ((2.0 * b.half[1] / ds).ceil() as usize).max(1);
        let (du, dw) = (2.0 * b.half[0] / nu as f32, 2.0 * b.half[1] / nw as f32);
        let tread = e.base + b.rise.1;
        for i in 0..nu {
            for j in 0..nw {
                let (u, w) = (
                    -b.half[0] + (i as f32 + 0.5) * du,
                    -b.half[1] + (j as f32 + 0.5) * dw,
                );
                let (x, z) = (
                    b.centre[0] + a[0] * u + n[0] * w,
                    b.centre[1] + a[1] * u + n[1] * w,
                );
                let d = f64::from(rel(x, z) - tread);
                let cellv = f64::from(du) * f64::from(dw);
                if d > 0.0 {
                    cut += d * cellv;
                } else {
                    fill -= d * cellv;
                }
            }
        }
    }
    if t <= 0.0 {
        // The ground just outside the sides, against the tread beside it.
        for r in &runs_of(&e.points, ends(&blocks)) {
            let n = ((r.len / 0.25).ceil() as usize).max(1);
            for k in 0..=n {
                let u = r.len * k as f32 / n as f32;
                let p = [r.from[0] + r.dir[0] * u, r.from[1] + r.dir[1] * u];
                let b = block_at(&blocks, [p[0] - r.out[0] * 0.01, p[1] - r.out[1] * 0.01]);
                let beside = rel(p[0] + r.out[0] * 0.05, p[1] + r.out[1] * 0.05);
                bank = bank.max(beside - (e.base + blocks[b].rise.1));
            }
        }
    } else {
        for c in &cheeks {
            for p in c {
                for s in &p.stations {
                    bank = bank.max(s.ground - p.tread);
                }
            }
        }
    }
    Ok(Built {
        geometry: g,
        lift,
        bottoms,
        set_in,
        hole,
        runs,
        cheeks,
        cut_m3: cut as f32,
        fill_m3: fill as f32,
        bank_m: bank.max(0.0),
        base: e.base,
    })
}

/// The plain side runs of an outline, for the bank beside a flight with no cheeks.
fn runs_of(outline: &[P2], ends: (P2, P2)) -> Vec<Run> {
    runs(outline, ends, 0.0).into_iter().flatten().collect()
}

impl Built {
    /// The treads for the ground queries, absolute.
    pub fn treads(&self) -> Vec<Tread> {
        self.geometry
            .blocks()
            .iter()
            .map(|b| Tread {
                centre: b.centre,
                axis: b.axis,
                half: b.half,
                level: self.lift + self.base + b.rise.1,
            })
            .collect()
    }

    /// The cheeks' sleepers: courses of whole beams over the tallest stretch, each kept where the
    /// wall reaches that high, so the top steps with the slope.
    pub fn beams(&self) -> Vec<Beam> {
        let t = self.set_in.thickness();
        let mut out = Vec::new();
        for (run, pieces) in self.runs.iter().zip(&self.cheeks) {
            let all = || pieces.iter().flat_map(|p| p.stations.iter());
            let lo = all().map(|s| s.lo).fold(f32::MAX, f32::min);
            let hi = all().map(|s| s.hi).fold(f32::MIN, f32::max).max(lo + 0.05);
            let courses = (((hi - lo) / bed::BEAM_H).round() as usize).max(1);
            let ch = (hi - lo) / courses as f32;
            let from = [run.from[0] + run.out[0] * t, run.from[1] + run.out[1] * t];
            let inward = [-run.out[0], -run.out[1]];
            let n = (((run.u.1 - run.u.0) / BEAM_LEN_M).ceil() as usize).max(1);
            let wall_at = |u: f32| -> (f32, f32) {
                // The wall's bottom and top at `u`: from the nearest station.
                let mut best = (f32::MAX, (lo, hi));
                for s in all() {
                    let su = (s.inner[0] - run.from[0]) * run.dir[0]
                        + (s.inner[1] - run.from[1]) * run.dir[1];
                    if (su - u).abs() < best.0 {
                        best = ((su - u).abs(), (s.lo, s.hi));
                    }
                }
                best.1
            };
            for c in 0..courses {
                let y = (
                    lo + c as f32 * ch + if c > 0 { bed::JOINT / 2.0 } else { 0.0 },
                    lo + (c + 1) as f32 * ch
                        - if c + 1 < courses {
                            bed::JOINT / 2.0
                        } else {
                            0.0
                        },
                );
                for j in 0..n {
                    let a = run.u.0 + (run.u.1 - run.u.0) * j as f32 / n as f32;
                    let b = run.u.0 + (run.u.1 - run.u.0) * (j + 1) as f32 / n as f32;
                    // The wall's lowest top and bottom along the beam.
                    let m = 8;
                    let (mut blo, mut bhi_min, mut bhi) = (f32::MAX, f32::MAX, f32::MIN);
                    for k in 0..=m {
                        let (l, h) = wall_at(a + (b - a) * k as f32 / m as f32);
                        blo = blo.min(l);
                        bhi_min = bhi_min.min(h);
                        bhi = bhi.max(h);
                    }
                    let mid = (y.0 + y.1) / 2.0;
                    // Kept where the wall reaches the beam's middle along most of it.
                    if mid >= bhi_min + (bhi - bhi_min) * 0.5 || y.1 <= blo + 0.02 {
                        continue;
                    }
                    out.push(Beam {
                        from,
                        dir: run.dir,
                        inward,
                        u: (
                            a + if j > 0 { bed::JOINT / 2.0 } else { 0.0 },
                            b - if j + 1 < n { bed::JOINT / 2.0 } else { 0.0 },
                        ),
                        y,
                        top: mid + ch >= bhi,
                    });
                }
            }
        }
        out
    }

    /// Draws the cheeks on faces 0 (`cheeks`) and 1 (`cheeks_rim`) of the sink.
    pub(crate) fn draw(&self, s: &mut dyn Sink) {
        let Some(style) = self.set_in.cheek_style.bed() else {
            return;
        };
        let t = self.set_in.thickness();
        if style == BedStyle::Sleepers {
            for b in self.beams() {
                bed::beam(s, &b, t);
            }
            return;
        }
        let c = |p: P2, y: f32| [p[0], y, p[1]];
        for (run, pieces) in self.runs.iter().zip(&self.cheeks) {
            let (out, d) = (run.out, run.dir);
            let (out3, d3) = ([out[0], 0.0, out[1]], [d[0], 0.0, d[1]]);
            let neg = |v: [f32; 3]| v.map(|x| -x);
            let push = |p: P2, k: f32| [p[0] + out[0] * k, p[1] + out[1] * k];
            for piece in pieces {
                let st = &piece.stations;
                let tread = piece.tread;
                // The wall's own top: under the coping or the folded edge when there is one.
                let top = |k: &Station| match style {
                    BedStyle::Masonry => (k.hi - COPING_H).max(k.lo),
                    BedStyle::Corten => (k.hi - FLANGE_H).max(k.lo),
                    _ => k.hi,
                };
                for w in st.windows(2) {
                    let (a, b) = (&w[0], &w[1]);
                    // The outer face, down to the ground and below it.
                    quad(
                        s,
                        WALL,
                        [
                            c(a.outer, a.lo),
                            c(b.outer, b.lo),
                            c(b.outer, top(b)),
                            c(a.outer, top(a)),
                        ],
                        out3,
                    );
                    // The inner face shows over the tread only.
                    let (ia, ib) = (top(a).max(tread), top(b).max(tread));
                    quad(
                        s,
                        WALL,
                        [
                            c(a.inner, tread),
                            c(b.inner, tread),
                            c(b.inner, ib),
                            c(a.inner, ia),
                        ],
                        neg(out3),
                    );
                    let up = [0.0, 1.0, 0.0];
                    let strip = |s: &mut dyn Sink,
                                 face: usize,
                                 p: [P2; 2],
                                 q: [P2; 2],
                                 ya: [f32; 2],
                                 normal: [f32; 3]| {
                        quad(
                            s,
                            face,
                            [
                                c(p[0], ya[0]),
                                c(p[1], ya[1]),
                                c(q[1], ya[1]),
                                c(q[0], ya[0]),
                            ],
                            normal,
                        );
                    };
                    let hi = [a.hi, b.hi];
                    let (oa, ob) = ([a.outer, b.outer], [a.inner, b.inner]);
                    match style {
                        BedStyle::Plain | BedStyle::Gabion => {
                            strip(s, RIM, oa, ob, hi, up);
                        }
                        BedStyle::Masonry => {
                            let ext = [push(a.outer, COPING_OUT), push(b.outer, COPING_OUT)];
                            let yt = [top(a), top(b)];
                            quad(
                                s,
                                RIM,
                                [
                                    c(ext[0], yt[0]),
                                    c(ext[1], yt[1]),
                                    c(ext[1], b.hi),
                                    c(ext[0], a.hi),
                                ],
                                out3,
                            );
                            quad(
                                s,
                                RIM,
                                [
                                    c(a.inner, yt[0].max(tread)),
                                    c(b.inner, yt[1].max(tread)),
                                    c(b.inner, b.hi.max(tread)),
                                    c(a.inner, a.hi.max(tread)),
                                ],
                                neg(out3),
                            );
                            strip(s, RIM, ext, ob, hi, up);
                            strip(s, WALL, ext, oa, yt, [0.0, -1.0, 0.0]);
                        }
                        BedStyle::Corten => {
                            let ext = [push(a.outer, FLANGE_OUT), push(b.outer, FLANGE_OUT)];
                            let fin = [push(a.inner, -FLANGE_IN), push(b.inner, -FLANGE_IN)];
                            let yt = [top(a), top(b)];
                            strip(s, RIM, ext, fin, hi, up);
                            strip(s, RIM, ext, fin, yt, [0.0, -1.0, 0.0]);
                            let lip = [(a.hi - LIP_H).max(a.lo), (b.hi - LIP_H).max(b.lo)];
                            quad(
                                s,
                                RIM,
                                [
                                    c(ext[0], lip[0]),
                                    c(ext[1], lip[1]),
                                    c(ext[1], b.hi),
                                    c(ext[0], a.hi),
                                ],
                                out3,
                            );
                        }
                        BedStyle::Sleepers => {}
                    }
                }
                if style == BedStyle::Gabion {
                    // Wire bars across the outer face.
                    let lowest = st.iter().map(|k| k.lo).fold(f32::MAX, f32::min);
                    let highest = st.iter().map(|k| k.hi).fold(f32::MIN, f32::max);
                    let mut y = (lowest / BAR_PITCH_M).ceil() * BAR_PITCH_M;
                    while y < highest {
                        for w in st.windows(2) {
                            let (a, b) = (&w[0], &w[1]);
                            let (lo, hi) = (a.lo.max(b.lo) + 0.03, a.hi.min(b.hi) - BAR_H - 0.03);
                            if y >= lo && y <= hi {
                                let (p, q) = (push(a.outer, BAR_OUT), push(b.outer, BAR_OUT));
                                quad(
                                    s,
                                    RIM,
                                    [c(p, y), c(q, y), c(q, y + BAR_H), c(p, y + BAR_H)],
                                    out3,
                                );
                            }
                        }
                        y += BAR_PITCH_M;
                    }
                }
            }
            // The ends of the wall, and where it steps between two treads.
            let cap = |s: &mut dyn Sink, st: &Station, y: (f32, f32), normal: [f32; 3]| {
                quad(
                    s,
                    WALL,
                    [
                        c(st.outer, y.0),
                        c(st.inner, y.0),
                        c(st.inner, y.1),
                        c(st.outer, y.1),
                    ],
                    normal,
                );
            };
            if let (Some(first), Some(last)) = (pieces.first(), pieces.last()) {
                let (a, z) = (&first.stations[0], &last.stations[last.stations.len() - 1]);
                cap(s, a, (a.lo, a.hi), neg(d3));
                cap(s, z, (z.lo, z.hi), d3);
            }
            for w in pieces.windows(2) {
                let (a, b) = (&w[0].stations, &w[1].stations);
                let (ea, sb) = (&a[a.len() - 1], &b[0]);
                if ea.hi > sb.hi + 1e-4 {
                    cap(s, ea, (sb.hi, ea.hi), d3);
                } else if sb.hi > ea.hi + 1e-4 {
                    cap(s, sb, (ea.hi, sb.hi), neg(d3));
                }
            }
        }
    }

    /// The earth moved, the cheeks built and what to tell about them.
    pub fn report(&self) -> Report {
        let (mut length, mut outer_a, mut inner_a, mut max_h) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for pieces in &self.cheeks {
            for p in pieces {
                for w in p.stations.windows(2) {
                    let (a, b) = (&w[0], &w[1]);
                    let l = (a.outer[0] - b.outer[0]).hypot(a.outer[1] - b.outer[1]);
                    let h = ((a.hi - a.lo) + (b.hi - b.lo)) / 2.0;
                    length += l;
                    outer_a += l * h;
                    // The inner face shows over the tread only.
                    let hi = ((a.hi - p.tread).max(0.0) + (b.hi - p.tread).max(0.0)) / 2.0;
                    inner_a += l * hi;
                    max_h = max_h.max(a.hi - a.lo);
                }
            }
        }
        let style = self.set_in.cheek_style.bed();
        let beams = (style == Some(BedStyle::Sleepers)).then(|| {
            let beams = self.beams();
            let length = beams.iter().map(|b| b.u.1 - b.u.0).sum();
            (beams.len(), length)
        });
        Report {
            goings: self.geometry.goings.clone(),
            cut_m3: self.cut_m3,
            fill_m3: self.fill_m3,
            cheek_length_m: length,
            outer_face_m2: outer_a,
            inner_face_m2: inner_a,
            max_height_m: max_h,
            beams,
            plate_area_m2: (style == Some(BedStyle::Corten)).then_some(outer_a),
        }
    }

    /// What to tell the person about the flight, besides the goings' own notes.
    pub fn notes(&self) -> Vec<String> {
        let mut out = Vec::new();
        let r = self.report();
        if r.max_height_m > ENGINEER_M {
            out.push(format!(
                "a cheek wall stands up to {:.2} m high; a wall over {ENGINEER_M} m needs an engineer's design",
                r.max_height_m
            ));
        }
        if self.set_in.cheek_style == CheekStyle::None && self.bank_m > 0.05 {
            out.push(format!(
                "no cheeks: the ground beside the steps rises up to {:.2} m above a tread, and nothing holds that bank back; give `cheek_style` a wall",
                self.bank_m
            ));
        }
        out
    }
}

impl Report {
    /// The report as `stairs.add`, `stairs.set` and `query.quantities` give it.
    pub fn json(&self, set_in: &SetIn) -> Value {
        let mut v = json!({
            "cheek_style": set_in.cheek_style,
            "thickness_m": set_in.thickness(),
            "goings_m": self.goings,
            "cut_m3": self.cut_m3,
            "fill_m3": self.fill_m3,
            "net_cut_m3": self.cut_m3 - self.fill_m3,
            "cheek_length_m": self.cheek_length_m,
            "cheek_outer_face_area_m2": self.outer_face_m2,
            "cheek_inner_face_area_m2": self.inner_face_m2,
            "cheek_max_height_m": self.max_height_m,
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

// --- In a project ----------------------------------------------------------------------------

/// The flights set into the slope of `project` as the ground queries see them: each is a zone
/// whose hole is cut out of the terrain and whose treads answer for the ground inside it.
/// `terraces` are the project's terrace zones, which count as ground; a flight is measured
/// against the original ground, not against another flight.
pub fn zones(project: &Project, terraces: &[Zone]) -> Vec<Zone> {
    let terrain = project.terrain.as_deref();
    let ground = |x: f32, z: f32| crate::terrace::ground_at(terrain, terraces, x, z);
    let cell = terrain.map_or(crate::terrace::DEFAULT_CELL_M, |t| t.cell_m);
    project
        .shapes
        .iter()
        .filter_map(|s| {
            let Shape::Extrusion(e) = s else { return None };
            let st = e.stairs.as_ref().filter(|_| e.closed)?;
            st.set_in?;
            let b = build(e, &ground, cell).ok()?;
            Some(Zone::new(
                e.id.clone(),
                e.points.clone(),
                b.hole.clone(),
                b.lift + e.base,
                b.treads(),
            ))
        })
        .collect()
}

/// The built flight `e` of `project`: measured against the terrain and the terraces.
pub fn build_in(project: &Project, e: &Extrusion) -> Option<Built> {
    let terrain = project.terrain.as_deref();
    let terraces = crate::terrace::zones(&project.shapes);
    let ground = |x: f32, z: f32| crate::terrace::ground_at(terrain, &terraces, x, z);
    let cell = terrain.map_or(crate::terrace::DEFAULT_CELL_M, |t| t.cell_m);
    build(e, &ground, cell).ok()
}

/// What a flight set into the slope costs, for `query.quantities` and the commands' answers;
/// `None` for any other shape.
pub fn report_for(project: &Project, e: &Extrusion) -> Option<(Report, Vec<String>)> {
    e.stairs.as_ref()?.set_in?;
    let b = build_in(project, e)?;
    let mut notes = b.geometry.warnings.clone();
    notes.extend(b.notes());
    Some((b.report(), notes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;
    use crate::shape::inside_outline;
    use crate::stairs::Stairs;
    use crate::terrain::Terrain;
    use std::sync::Arc;

    /// A 40 x 30 m terrain (0.5 m cells) whose ground is `h(x, z)`.
    fn project(h: impl Fn(f32, f32) -> f32) -> Project {
        let mut t = Terrain::flat([0.0, 0.0], 40.0, 30.0, 0.5, "lawn").unwrap();
        let w = t.width as usize + 1;
        for (i, v) in t.heights.iter_mut().enumerate() {
            *v = h((i % w) as f32 * 0.5, (i / w) as f32 * 0.5);
        }
        Project {
            terrain: Some(Arc::new(t)),
            ..Project::default()
        }
    }

    fn sleepers() -> SetIn {
        SetIn {
            cheek_style: CheekStyle::Sleepers,
            ..SetIn::default()
        }
    }

    /// A straight flight from `from` (bottom centre) to `to` (top centre).
    fn flight(from: P2, to: P2, width: f32, height: f32, set_in: SetIn) -> Extrusion {
        let len = (to[0] - from[0]).hypot(to[1] - from[1]);
        let n = [
            -(to[1] - from[1]) / len * width / 2.0,
            (to[0] - from[0]) / len * width / 2.0,
        ];
        let at = |p: P2, k: f32| [p[0] + n[0] * k, p[1] + n[1] * k];
        Extrusion {
            id: "S1".into(),
            name: "Steps".into(),
            layer: "shapes".into(),
            points: vec![at(from, -1.0), at(from, 1.0), at(to, 1.0), at(to, -1.0)],
            curves: Default::default(),
            smooth: false,
            closed: true,
            base: 0.0,
            height,
            thickness: 0.2,
            extensions: Default::default(),
            house: None,
            fence: None,
            stairs: Some(Stairs {
                set_in: Some(set_in),
                ..Stairs::new(2)
            }),
            raised_bed: None,
            furniture: None,
        }
    }

    fn with(mut p: Project, e: &Extrusion) -> Project {
        p.shapes.push(Shape::Extrusion(e.clone()));
        p
    }

    #[test]
    fn on_a_uniform_slope_the_goings_are_equal_and_match_it() {
        let p = project(|x, _| x / 3.0);
        let e = flight([6.0, 10.0], [12.0, 10.0], 1.2, 2.0, sleepers());
        let b = build_in(&p, &e).unwrap();
        let g = &b.geometry;
        assert_eq!(g.steps, 12);
        assert_eq!(g.goings.len(), 12);
        for (k, going) in g.goings.iter().enumerate() {
            assert!((going - 0.5).abs() < 2e-3, "step {k}: {going}");
        }
        // 1 in 3 is gentler than a comfortable stair: the going and Blondel both fail.
        assert!(
            g.warnings.iter().any(|w| w.contains("uncomfortable")),
            "{:?}",
            g.warnings
        );
        // 1 in 1.7 gives goings near 0.29: comfortable, nothing to say.
        let p = project(|x, _| x / 1.7);
        let e = flight([6.0, 10.0], [9.5, 10.0], 1.2, 2.04, sleepers());
        let g = build_in(&p, &e).unwrap().geometry;
        assert!(g.warnings.is_empty(), "{:?}", g.warnings);
        assert!(
            g.goings[..11].iter().all(|v| (v - 0.289).abs() < 0.02),
            "{:?}",
            g.goings
        );
    }

    #[test]
    fn on_a_convex_profile_the_goings_vary_and_every_tread_lies_within_a_riser_of_the_ground() {
        let ground = |x: f32, _: f32| 0.03 * (x - 4.0).powi(2);
        let p = project(ground);
        let e = flight([6.0, 10.0], [14.0, 10.0], 1.2, 2.88, sleepers());
        let b = build_in(&p, &e).unwrap();
        let g = &b.geometry;
        assert_eq!(g.steps, 17);
        let (first, last) = (g.goings[0], g.goings[g.steps - 2]);
        assert!(first > 3.0 * last, "{first} {last}: {:?}", g.goings);
        assert!(g.goings.iter().all(|v| (0.25..=2.0).contains(v)));
        assert!((g.goings.iter().sum::<f32>() - 8.0).abs() < 1e-3);
        let us = crate::stairs::offsets(&g.goings);
        for k in 1..g.steps {
            let x = 6.0 + (us[k - 1] + us[k]) / 2.0;
            let tread = b.lift + k as f32 * g.riser;
            assert!(
                (ground(x, 10.0) - tread).abs() <= g.riser + 1e-3,
                "step {k}: ground {} tread {tread}",
                ground(x, 10.0)
            );
        }
    }

    #[test]
    fn a_going_is_held_within_its_limits_and_warns() {
        // 1 in 0.5: the ground climbs a riser in 0.085 m.
        let steep = fit_goings(&|s| 2.0 * s, 6.0, 12, 0.17, 0.0, "").unwrap();
        assert!(steep.goings[..11].iter().all(|g| (g - 0.25).abs() < 1e-4));
        assert!((steep.goings.iter().sum::<f32>() - 6.0).abs() < 1e-4);
        assert!(steep.notes.iter().any(|n| n.contains("steeper")));
        // 1 in 50: a riser takes 8.5 m of ground.
        let flat = fit_goings(&|s| 0.02 * s, 40.0, 10, 0.17, 0.0, "").unwrap();
        assert!(flat.goings[..9].iter().all(|g| (g - 2.0).abs() < 1e-4));
        assert!(flat.notes.iter().any(|n| n.contains("flatter")));
        // Too short for the slope: the goings give way to the footprint.
        let tight = fit_goings(&|s| 0.4 * s, 4.0, 12, 0.17, 0.0, "flight 1: ").unwrap();
        assert!((tight.goings.iter().sum::<f32>() - 4.0).abs() < 1e-4);
        assert!(tight.goings.iter().all(|g| *g >= 0.25 - 1e-4));
        assert!(
            tight
                .notes
                .iter()
                .any(|n| n.starts_with("flight 1: ") && n.contains("too short"))
        );
        // No room for the minimum going at all.
        assert!(fit_goings(&|s| s, 2.0, 12, 0.17, 0.0, "").is_err());
    }

    #[test]
    fn the_terrain_has_no_triangle_inside_the_flights_hole_and_the_ground_there_is_the_tread() {
        let p = with(
            project(|x, _| x / 3.0),
            &flight([6.0, 10.0], [12.0, 10.0], 1.2, 2.0, sleepers()),
        );
        let zones = crate::terrace::project_zones(&p);
        assert_eq!(zones.len(), 1);
        let hole = &zones[0].hole;
        // Widened at the sides by the cheek (0.15 m), not at the bottom and the top.
        let (lo, hi) = hole
            .iter()
            .fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), q| {
                (
                    [lo[0].min(q[0]), lo[1].min(q[1])],
                    [hi[0].max(q[0]), hi[1].max(q[1])],
                )
            });
        assert!(
            (lo[0] - 6.0).abs() < 1e-4 && (hi[0] - 12.0).abs() < 1e-4,
            "{lo:?} {hi:?}"
        );
        assert!(
            (lo[1] - 9.25).abs() < 1e-4 && (hi[1] - 10.75).abs() < 1e-4,
            "{lo:?} {hi:?}"
        );
        let t = p.terrain.as_deref().unwrap();
        let m = crate::terrain_chunk_mesh_holes(t, 0, 0, 1, None, &crate::terrace::holes(&zones));
        let mut outside = 0;
        for tri in m.indices.chunks(3) {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| m.positions[i as usize]);
            let mid = [(a[0] + b[0] + c[0]) / 3.0, (a[2] + b[2] + c[2]) / 3.0];
            assert!(
                !inside_outline(mid, hole),
                "a triangle lies inside the hole: {mid:?}"
            );
            outside += 1;
        }
        assert!(outside > 100);
        // The ground in the flight is the tread under it; beside it, the terrain's.
        let ground = crate::terrace::ground_fn(&p);
        let b = build_in(&p, &flight([6.0, 10.0], [12.0, 10.0], 1.2, 2.0, sleepers())).unwrap();
        for t in b.treads() {
            let at = ground(t.centre[0], t.centre[1]).unwrap();
            assert!((at - t.level).abs() < 1e-5, "{at} {}", t.level);
        }
        assert!((ground(9.0, 12.0).unwrap() - 3.0).abs() < 1e-4);
        // ...and the tread levels rise one riser at a time from the bottom.
        let levels: Vec<f32> = b.treads().iter().map(|t| t.level).collect();
        assert!((levels[0] - (2.0 + 2.0 / 12.0)).abs() < 1e-4, "{levels:?}");
        assert!((levels[11] - 4.0).abs() < 1e-4, "{levels:?}");
    }

    #[test]
    fn the_cheek_tops_follow_the_ground_and_stay_under_the_treads_where_it_is_lower() {
        // The ground climbs along the flight and falls away to the right (+z) of it.
        let p = project(|x, z| x / 3.0 + (10.0 - z) * 0.2);
        let e = flight([6.0, 10.0], [12.0, 10.0], 1.2, 2.0, SetIn::default());
        let b = build_in(&p, &e).unwrap();
        assert_eq!(b.runs.len(), 2);
        let coping = b.set_in.coping_m;
        let (mut holds, mut below) = (0, 0);
        for (run, pieces) in b.runs.iter().zip(&b.cheeks) {
            for piece in pieces {
                for s in &piece.stations {
                    // The outer face stands the cheek's thickness off the flight's edge.
                    assert!((s.outer[1] - 10.0).abs() > 0.6 + 0.2 - 1e-3, "{s:?}");
                    assert!(
                        (s.ground - (s.outer[0] / 3.0 + (10.0 - s.outer[1]) * 0.2 - b.lift)).abs()
                            < 1e-3
                    );
                    if s.ground >= piece.tread {
                        holds += 1;
                        assert!((s.hi - (s.ground + coping)).abs() < 1e-5, "{s:?}");
                    } else {
                        below += 1;
                        assert!(s.hi <= piece.tread - BELOW_TREAD_M + 1e-5, "{s:?}");
                    }
                    assert!(s.lo < s.ground.min(piece.tread) && s.lo < s.hi);
                }
            }
            assert!(run.len > 5.9);
        }
        assert!(holds > 10 && below > 10, "{holds} {below}");
    }

    #[test]
    fn a_flight_with_no_cheeks_cuts_the_terrain_to_its_outline_and_warns_of_a_bank() {
        let p = project(|x, z| x / 3.0 + (z - 10.0).abs() * 0.5);
        let e = flight(
            [6.0, 10.0],
            [12.0, 10.0],
            1.2,
            2.0,
            SetIn {
                cheek_style: CheekStyle::None,
                ..SetIn::default()
            },
        );
        let b = build_in(&p, &e).unwrap();
        assert_eq!(b.hole, e.points);
        assert!(b.cheeks.is_empty());
        assert!(b.bank_m > 0.2, "{}", b.bank_m);
        assert!(
            b.notes().iter().any(|n| n.contains("no cheeks")),
            "{:?}",
            b.notes()
        );
        // With a wall there is nothing to warn of.
        let walled = flight([6.0, 10.0], [12.0, 10.0], 1.2, 2.0, sleepers());
        let notes = build_in(&p, &walled).unwrap().notes();
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn every_cheek_style_draws_and_a_turned_flight_works() {
        let ground = |x: f32, z: f32| (x + z) / 5.0;
        for style in [
            CheekStyle::Plain,
            CheekStyle::Masonry,
            CheekStyle::Sleepers,
            CheekStyle::Corten,
            CheekStyle::Gabion,
        ] {
            let p = project(ground);
            let mut e = flight([6.0, 6.0], [6.0, 12.0], 1.2, 1.2, SetIn::default());
            let path = vec![[6.0, 6.0], [6.0, 12.0], [12.0, 12.0]];
            e.points = crate::stairs::turned_outline(&path, 1.2).unwrap();
            e.height = 2.4;
            e.stairs = Some(Stairs {
                path: Some(path),
                width_m: Some(1.2),
                set_in: Some(SetIn {
                    cheek_style: style,
                    ..SetIn::default()
                }),
                ..Stairs::new(2)
            });
            let p = with(p, &e);
            let b = build_in(&p, &e).unwrap();
            let t = b.geometry.turned.as_ref().unwrap();
            assert_eq!(t.flights.len(), 2);
            assert_eq!(t.flights.iter().map(|f| f.steps).sum::<usize>(), 14);
            for f in &t.flights {
                assert_eq!(f.goings.len(), f.steps);
                assert!((f.goings.iter().sum::<f32>() - f.run_m).abs() < 1e-3);
            }
            assert_eq!(b.runs.len(), 4, "the sides and the landing, not the ends");
            assert_eq!(b.treads().len(), 15);
            // The mesh has all five faces and the cheeks have triangles.
            let (m, _) = crate::plant_ground::placed_shape_mesh(
                &p,
                &Shape::Extrusion(e.clone()),
                &Default::default(),
                &[],
            )
            .unwrap();
            let names: Vec<&str> = m.faces.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(
                names,
                [
                    "S1:treads",
                    "S1:risers",
                    "S1:sides",
                    "S1:cheeks",
                    "S1:cheeks_rim"
                ]
            );
            for face in [3u32, 4] {
                assert!(
                    m.triangle_faces.contains(&face),
                    "{style:?} face {face} is empty"
                );
            }
            assert!(m.positions.iter().all(|p| p.iter().all(|c| c.is_finite())));
            let r = b.report();
            assert!(r.cheek_length_m > 10.0 && r.cut_m3 > 0.0);
        }
    }

    #[test]
    fn set_in_survives_a_round_trip_and_old_stairs_do_not_have_it() {
        let si = SetIn {
            cheek_style: CheekStyle::Gabion,
            thickness_m: Some(0.3),
            coping_m: 0.2,
        };
        let v = serde_json::to_value(si).unwrap();
        assert_eq!(v["cheek_style"], "gabion");
        assert!(v["thickness_m"].is_number() && v["coping_m"].is_number());
        assert_eq!(serde_json::from_value::<SetIn>(v).unwrap(), si);
        let plain = serde_json::to_value(SetIn::default()).unwrap();
        assert_eq!(plain, json!({"cheek_style": "plain"}));
        assert_eq!(
            serde_json::from_value::<SetIn>(json!({})).unwrap(),
            SetIn::default()
        );
        assert!(serde_json::from_value::<SetIn>(json!({"cheeks": 1})).is_err());
        let old: Stairs = serde_json::from_value(json!({"up_edge": 2})).unwrap();
        assert!(old.set_in.is_none());
        assert!(serde_json::to_value(&old).unwrap().get("set_in").is_none());
        assert!(
            SetIn {
                coping_m: 2.0,
                ..SetIn::default()
            }
            .check()
            .is_err()
        );
        assert!(
            SetIn {
                thickness_m: Some(0.5),
                ..sleepers()
            }
            .check()
            .is_err()
        );
    }
}
