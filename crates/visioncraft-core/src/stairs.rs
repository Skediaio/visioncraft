//! Steps and stairs: what turns a closed extrusion with a rectangular footprint into a flight of
//! solid steps, drawn by `shape_mesh` with stable face names so materials attach:
//! `<id>:treads`, `<id>:risers`, `<id>:sides` (both flanks, the back and the underside).
//!
//! `base` is the bottom of the flight and `base + height` the level you arrive on. `up_edge`
//! names the outline edge (point `i` to point `i + 1`) at the top; the bottom is the opposite
//! edge. The flight runs between the midpoints of those two edges.
//!
//! Every step is a block from the bottom up (no floating slabs); the blocks sit side by side, so
//! the mesh is closed and its volume is the stairs' own. The step count is
//! `round(height / riser_m)` and the risers are all `height / count` high. With a `tread_m`
//! the treads are that deep and a footprint longer than the flight ends in a landing (the top
//! tread runs on to the back); without one the treads share the footprint evenly.
//!
//! A flight that turns has a `path` (the centreline: bottom centre, one or two corners, top
//! centre) and a `width_m`; its outline is derived from them ([`turned_outline`]). At each
//! corner the flight turns 90 degrees on a square landing of width x width centred on the
//! corner. Two corners at least one width apart make a U (or an S) stair: the middle segment is
//! one flat landing, a half-landing when the corners are exactly a width apart, with a well
//! between the two flights when they are further apart. The steps are shared over the flights
//! in proportion to their clear run (the segment less half a landing at each corner end); a
//! landing lies at the height of the last step of the flight that reaches it, so it is that
//! step with a long tread. Every step and landing is a solid block from the ground up.

use crate::house::Sink;
use crate::setin::{self, SetIn};
use serde::{Deserialize, Serialize};

type P2 = [f32; 2];

/// Faces a flight has, in this order.
pub const STAIR_FACES: [&str; 3] = ["treads", "risers", "sides"];
const TREADS: usize = 0;
const RISERS: usize = 1;
const SIDES: usize = 2;

/// The riser height a flight starts with (metres).
pub const DEFAULT_RISER_M: f32 = 0.17;
/// The tread depth the Blondel rule pairs with the default riser (metres).
pub const DEFAULT_TREAD_M: f32 = 0.30;
/// Blondel's rule for a comfortable step: `2 * riser + tread` lies in this range (metres).
pub const BLONDEL_M: (f32, f32) = (0.60, 0.65);
const MAX_STEPS: usize = 80;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stairs {
    /// The outline edge (point `up_edge` to the next one) at the top, where you arrive.
    pub up_edge: usize,
    /// The height of a riser to aim for, 0.05..=0.30; the step count is `round(height / riser_m)`.
    /// Default [`DEFAULT_RISER_M`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub riser_m: Option<f32>,
    /// The depth of a tread, 0.15..=0.60. Without it the treads share the footprint evenly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tread_m: Option<f32>,
    /// The centreline of a flight that turns: 3 or 4 world points, the bottom centre, one or two
    /// corners and the top centre. The outline is derived from it; `up_edge` is not used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<P2>>,
    /// The width of a flight that turns (the outline cannot tell it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_m: Option<f32>,
    /// A flight set into the slope it climbs (see [`crate::setin`]): the steps are spaced along
    /// the ground, the footprint is cut out of the terrain and cheek walls hold the earth beside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_in: Option<SetIn>,
}

/// One flight of a stair that turns: the steps it climbs and the run they have.
#[derive(Clone, Debug, PartialEq)]
pub struct Flight {
    pub steps: usize,
    /// The clear run: the segment less half a landing at each corner end.
    pub run_m: f32,
    pub tread_m: f32,
    /// The depth of each step's tread, bottom to top (the last one runs on to the landing).
    pub goings: Vec<f32>,
}

/// A landing of a stair that turns.
#[derive(Clone, Debug, PartialEq)]
pub struct Landing {
    /// Width across the flights and the long side.
    pub size_m: [f32; 2],
    /// Height above the bottom of the flight.
    pub height_m: f32,
}

/// One solid block of a stair that turns: a step or a piece of a landing, from the ground up to
/// `rise.1` above the bottom of the flight.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub centre: P2,
    /// Unit direction of the half extent `half[0]`; the other one lies across it.
    pub axis: P2,
    pub half: [f32; 2],
    /// Heights above the bottom of the flight: where the riser starts and where the tread is.
    pub rise: (f32, f32),
    /// A step (its front wall towards `-axis` is a riser) or a piece of a landing.
    pub step: bool,
}

impl Block {
    /// The four plan corners.
    pub fn corners(&self) -> [P2; 4] {
        let (a, n) = (self.axis, [-self.axis[1], self.axis[0]]);
        let [hu, hw] = self.half;
        let p = |u: f32, w: f32| {
            [
                self.centre[0] + a[0] * u + n[0] * w,
                self.centre[1] + a[1] * u + n[1] * w,
            ]
        };
        [p(-hu, -hw), p(hu, -hw), p(hu, hw), p(-hu, hw)]
    }
}

/// The parts of a stair that turns.
#[derive(Clone, Debug, PartialEq)]
pub struct Turned {
    pub flights: Vec<Flight>,
    pub landings: Vec<Landing>,
    /// Bottom to top: the steps of the first flight, the landing, the steps of the next.
    pub blocks: Vec<Block>,
}

/// Where a flight lies and how its steps are cut.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    /// The middle of the bottom edge.
    pub origin: P2,
    /// Unit direction up the flight.
    pub dir: P2,
    /// From the bottom edge to the top edge.
    pub length: f32,
    pub width: f32,
    pub steps: usize,
    /// Height of one riser.
    pub riser: f32,
    /// Depth of one tread (the top one runs on to the back when there is a landing); the mean
    /// going of a flight set into the slope.
    pub tread: f32,
    /// The depth of every step's tread, bottom to top; for a stair that turns, flight after
    /// flight. All the same but the last one unless the flight is set into the slope.
    pub goings: Vec<f32>,
    /// Remarks that do not stop the flight being drawn.
    pub warnings: Vec<String>,
    /// The flights, landings and blocks of a stair that turns. For one, `origin` and `dir` are
    /// the bottom and the first direction, `length` is the length of the path and `tread` the
    /// mean going.
    pub turned: Option<Turned>,
}

impl Geometry {
    /// The steps and clear run of each flight: one for a straight flight.
    pub fn flights(&self) -> Vec<(usize, f32)> {
        match &self.turned {
            Some(t) => t.flights.iter().map(|f| (f.steps, f.run_m)).collect(),
            None => vec![(self.steps, self.length)],
        }
    }

    /// Every step and landing piece as a block, bottom to top.
    pub fn blocks(&self) -> Vec<Block> {
        if let Some(t) = &self.turned {
            return t.blocks.clone();
        }
        let us = offsets(&self.goings);
        (1..=self.steps)
            .map(|k| {
                let (u0, u1) = (us[k - 1], us[k]);
                let mid = (u0 + u1) / 2.0;
                Block {
                    centre: [
                        self.origin[0] + self.dir[0] * mid,
                        self.origin[1] + self.dir[1] * mid,
                    ],
                    axis: self.dir,
                    half: [(u1 - u0) / 2.0, self.width / 2.0],
                    rise: ((k - 1) as f32 * self.riser, k as f32 * self.riser),
                    step: true,
                }
            })
            .collect()
    }
}

/// Where each step starts along its flight: the running sum of the goings, with the total last.
pub(crate) fn offsets(goings: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0];
    for g in goings {
        out.push(out.last().copied().unwrap_or(0.0) + g);
    }
    out
}

/// The goings of `n` equal treads of `tread` m on a run of `run` m: the last one runs on to the
/// end.
fn even_goings(n: usize, tread: f32, run: f32) -> Vec<f32> {
    let mut v = vec![tread; n];
    v[n - 1] = run - (n - 1) as f32 * tread;
    v
}

impl Stairs {
    pub fn new(up_edge: usize) -> Self {
        Self {
            up_edge,
            riser_m: None,
            tread_m: None,
            path: None,
            width_m: None,
            set_in: None,
        }
    }

    /// The riser height to aim for, after checking `height`, `riser_m` and `tread_m`.
    fn check_numbers(&self, height: f32) -> Result<f32, String> {
        if !(height.is_finite() && (0.05..=6.0).contains(&height)) {
            return Err("stairs need a `height` within 0.05..=6 m".into());
        }
        let riser_m = self.riser_m.unwrap_or(DEFAULT_RISER_M);
        if !(riser_m.is_finite() && (0.05..=0.30).contains(&riser_m)) {
            return Err("`riser_m` must be within 0.05..=0.30 m".into());
        }
        if let Some(t) = self.tread_m
            && !(t.is_finite() && (0.15..=0.60).contains(&t))
        {
            return Err("`tread_m` must be within 0.15..=0.60 m".into());
        }
        Ok(riser_m)
    }

    /// The flight on a 4-point rectangular `outline` rising `height` (or, with a `path`, the
    /// turned flight on its derived outline); the reason if it cannot be drawn.
    pub fn geometry(&self, outline: &[P2], height: f32) -> Result<Geometry, String> {
        self.geometry_with(outline, height, None)
    }

    /// [`Stairs::geometry`] with the ground: `ground(x, z)` is its height above the bottom of the
    /// flight. A flight that is set into the slope ([`SetIn`]) spaces its steps along it; without
    /// ground (or without `set_in`) the treads are equal.
    pub fn geometry_with(
        &self,
        outline: &[P2],
        height: f32,
        ground: Option<&dyn Fn(f32, f32) -> f32>,
    ) -> Result<Geometry, String> {
        let ground = ground.filter(|_| self.set_in.is_some());
        if let Some(path) = &self.path {
            return self.turned_geometry(path, outline, height, ground);
        }
        if outline.len() != 4 {
            return Err("stairs need a 4-point rectangular outline".into());
        }
        if self.up_edge >= 4 {
            return Err("`up_edge` must be 0..=3".into());
        }
        let riser_m = self.check_numbers(height)?;
        let i = self.up_edge;
        let (a, b) = (outline[i], outline[(i + 1) % 4]);
        let (c, d) = (outline[(i + 2) % 4], outline[(i + 3) % 4]);
        let dist = |p: P2, q: P2| (q[0] - p[0]).hypot(q[1] - p[1]);
        let (width, side) = (dist(a, b), dist(b, c));
        let tol = 0.02;
        let dot = (b[0] - a[0]) * (c[0] - b[0]) + (b[1] - a[1]) * (c[1] - b[1]);
        if width < 0.2
            || side < 0.15
            || (dist(c, d) - width).abs() > tol
            || (dist(d, a) - side).abs() > tol
            || (dot / (width * side)).abs() > 0.02
        {
            return Err(
                "the stairs' outline must be a rectangle at least 0.2 m wide and 0.15 m long"
                    .into(),
            );
        }
        let top = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let origin = [(c[0] + d[0]) / 2.0, (c[1] + d[1]) / 2.0];
        let length = dist(origin, top);
        let dir = [(top[0] - origin[0]) / length, (top[1] - origin[1]) / length];
        let steps = ((height / riser_m).round() as usize).max(1);
        if steps > MAX_STEPS {
            return Err(format!("{steps} steps is too many (at most {MAX_STEPS})"));
        }
        let riser = height / steps as f32;
        if riser > 0.4 {
            return Err(format!(
                "a single riser of {riser:.2} m is too tall; give a lower `height`"
            ));
        }
        let mut warnings = Vec::new();
        if let Some(ground) = ground {
            let profile = |s: f32| ground(origin[0] + dir[0] * s, origin[1] + dir[1] * s);
            let fit = setin::fit_goings(&profile, length, steps, riser, 0.0, "")?;
            warnings.extend(fit.notes);
            if self.tread_m.is_some() {
                warnings.push(
                    "`tread_m` is not used by a flight set into the slope: the goings follow the ground"
                        .into(),
                );
            }
            return Ok(Geometry {
                origin,
                dir,
                length,
                width,
                steps,
                riser,
                tread: length / steps as f32,
                goings: fit.goings,
                warnings,
                turned: None,
            });
        }
        let tread = match self.tread_m {
            Some(t) if t * steps as f32 <= length + 1e-4 => t,
            Some(t) => {
                warnings.push(format!(
                    "the footprint is too short for {steps} treads of {t:.2} m ({length:.2} m long); \
                     the treads are {:.2} m deep",
                    length / steps as f32
                ));
                length / steps as f32
            }
            None => length / steps as f32,
        };
        if tread < 0.1 {
            return Err(format!(
                "the footprint is too short for {steps} steps: {tread:.2} m a tread"
            ));
        }
        let blondel = 2.0 * riser + tread;
        if !(BLONDEL_M.0 - 1e-3..=BLONDEL_M.1 + 1e-3).contains(&blondel) {
            warnings.push(format!(
                "uncomfortable steps: 2 x riser {riser:.3} + tread {tread:.3} = {blondel:.2} m, \
                 Blondel's rule wants {:.2}..{:.2} m",
                BLONDEL_M.0, BLONDEL_M.1
            ));
        }
        Ok(Geometry {
            origin,
            dir,
            length,
            width,
            steps,
            riser,
            tread,
            goings: even_goings(steps, tread, length),
            warnings,
            turned: None,
        })
    }

    fn turned_geometry(
        &self,
        path: &[P2],
        outline: &[P2],
        height: f32,
        ground: Option<&dyn Fn(f32, f32) -> f32>,
    ) -> Result<Geometry, String> {
        let width = self
            .width_m
            .ok_or("a flight with a `path` needs its `width_m`")?;
        let riser_m = self.check_numbers(height)?;
        let lay = layout(path, width)?;
        let derived = lay.outline()?;
        if derived.len() != outline.len()
            || derived
                .iter()
                .zip(outline)
                .any(|(a, b)| (a[0] - b[0]).abs() > 1e-3 || (a[1] - b[1]).abs() > 1e-3)
        {
            return Err(
                "the outline of a flight that turns comes from its `path`; change it with stairs.set"
                    .into(),
            );
        }
        let total = ((height / riser_m).round() as usize).max(1);
        if total > MAX_STEPS {
            return Err(format!("{total} steps is too many (at most {MAX_STEPS})"));
        }
        let n_flights = lay.flights.len();
        if total < n_flights {
            return Err(format!(
                "{total} steps are too few for {n_flights} flights: give a higher `height` or a lower `riser_m`"
            ));
        }
        let riser = height / total as f32;
        if riser > 0.4 {
            return Err(format!(
                "a single riser of {riser:.2} m is too tall; give a lower `height`"
            ));
        }
        let runs: Vec<f32> = lay.flights.iter().map(|f| f.run).collect();
        // A flight set into the slope takes steps in proportion to the ground it climbs.
        let profile = |lf: &FlightRun| {
            let (start, dir) = (lf.start, lf.dir);
            move |g: &dyn Fn(f32, f32) -> f32, s: f32| {
                g(start[0] + dir[0] * s, start[1] + dir[1] * s)
            }
        };
        let weights: Vec<f32> = match ground {
            Some(g) => lay
                .flights
                .iter()
                .map(|lf| {
                    let at = profile(lf);
                    (at(g, lf.run) - at(g, 0.0)).max(0.02)
                })
                .collect(),
            None => runs.clone(),
        };
        let counts = split_steps(total, &weights);
        let mut warnings = Vec::new();
        let mut flights = Vec::new();
        let mut done_steps = 0usize;
        for (i, (&n, &run)) in counts.iter().zip(&runs).enumerate() {
            if let Some(g) = ground {
                let at = profile(&lay.flights[i]);
                let label = format!("flight {}: ", i + 1);
                let fit = setin::fit_goings(
                    &|s| at(g, s),
                    run,
                    n,
                    riser,
                    done_steps as f32 * riser,
                    &label,
                )?;
                warnings.extend(fit.notes);
                done_steps += n;
                flights.push(Flight {
                    steps: n,
                    run_m: run,
                    tread_m: run / n as f32,
                    goings: fit.goings,
                });
                continue;
            }
            done_steps += n;
            let tread = match self.tread_m {
                Some(t) if t * n as f32 <= run + 1e-4 => t,
                Some(t) => {
                    warnings.push(format!(
                        "flight {}: the run is too short for {n} treads of {t:.2} m ({run:.2} m long); \
                         the treads are {:.2} m deep",
                        i + 1,
                        run / n as f32
                    ));
                    run / n as f32
                }
                None => run / n as f32,
            };
            if tread < 0.1 {
                return Err(format!(
                    "flight {} is too short for {n} steps: {tread:.2} m a tread",
                    i + 1
                ));
            }
            let blondel = 2.0 * riser + tread;
            if !(BLONDEL_M.0 - 1e-3..=BLONDEL_M.1 + 1e-3).contains(&blondel) {
                warnings.push(format!(
                    "flight {}: uncomfortable steps: 2 x riser {riser:.3} + tread {tread:.3} = {blondel:.2} m, \
                     Blondel's rule wants {:.2}..{:.2} m",
                    i + 1,
                    BLONDEL_M.0,
                    BLONDEL_M.1
                ));
            }
            flights.push(Flight {
                steps: n,
                run_m: run,
                tread_m: tread,
                goings: even_goings(n, tread, run),
            });
        }
        // Blocks: the steps of each flight, the landing after the first one.
        let hw = width / 2.0;
        let mut blocks = Vec::new();
        let mut landings = Vec::new();
        let mut done = 0usize;
        for (i, (f, lf)) in flights.iter().zip(&lay.flights).enumerate() {
            let us = offsets(&f.goings);
            for j in 1..=f.steps {
                let u0 = us[j - 1];
                let u1 = if j == f.steps { f.run_m } else { us[j] };
                let mid = (u0 + u1) / 2.0;
                blocks.push(Block {
                    centre: [lf.start[0] + lf.dir[0] * mid, lf.start[1] + lf.dir[1] * mid],
                    axis: lf.dir,
                    half: [(u1 - u0) / 2.0, hw],
                    rise: ((done + j - 1) as f32 * riser, (done + j) as f32 * riser),
                    step: true,
                });
            }
            done += f.steps;
            if i == 0 {
                let top = done as f32 * riser;
                for r in &lay.landing {
                    blocks.push(Block {
                        centre: r.centre,
                        axis: r.axis,
                        half: r.half,
                        rise: (top - riser, top),
                        step: false,
                    });
                }
                landings.push(Landing {
                    size_m: lay.landing_size,
                    height_m: top,
                });
            }
        }
        let going = flights.iter().map(|f| f.run_m).sum::<f32>() / total as f32;
        let goings = flights.iter().flat_map(|f| f.goings.clone()).collect();
        Ok(Geometry {
            origin: lay.q[0],
            dir: lay.d[0],
            length: lay.len.iter().sum(),
            width,
            steps: total,
            riser,
            tread: going,
            goings,
            warnings,
            turned: Some(Turned {
                flights,
                landings,
                blocks,
            }),
        })
    }
}

/// `total` steps over flights of the given clear `runs`, in proportion, at least one each
/// (largest remainder).
fn split_steps(total: usize, runs: &[f32]) -> Vec<usize> {
    let sum: f32 = runs.iter().sum();
    let ideal: Vec<f32> = runs.iter().map(|r| total as f32 * r / sum).collect();
    let mut n: Vec<usize> = ideal.iter().map(|x| (x.floor() as usize).max(1)).collect();
    while n.iter().sum::<usize>() < total {
        // The flight furthest under its share gets the next step.
        let i = (0..n.len())
            .max_by(|&a, &b| (ideal[a] - n[a] as f32).total_cmp(&(ideal[b] - n[b] as f32)))
            .expect("a flight");
        n[i] += 1;
    }
    while n.iter().sum::<usize>() > total {
        // Only the minimum of one step each can overshoot: take from the largest flight.
        let i = (0..n.len())
            .filter(|&i| n[i] > 1)
            .max_by(|&a, &b| (n[a] as f32 - ideal[a]).total_cmp(&(n[b] as f32 - ideal[b])))
            .expect("a flight with spare steps");
        n[i] -= 1;
    }
    n
}

/// A flight's clear stretch of a path segment.
struct FlightRun {
    start: P2,
    dir: P2,
    run: f32,
}

/// A plan rectangle.
struct PlanRect {
    centre: P2,
    axis: P2,
    half: [f32; 2],
}

/// A path with its corners snapped to right angles, cut into flights and landing pieces.
struct Layout {
    /// The snapped path points.
    q: Vec<P2>,
    /// Unit directions and lengths of the segments.
    d: Vec<P2>,
    len: Vec<f32>,
    flights: Vec<FlightRun>,
    /// The landing as one to three touching rectangles.
    landing: Vec<PlanRect>,
    /// Width across the flights and the long side of the landing.
    landing_size: [f32; 2],
    width: f32,
}

/// How far from a right angle a corner may be (sin 1 degree).
const RIGHT_ANGLE_TOL: f32 = 0.017_452_4;
/// The shortest clear run a flight needs (metres).
const MIN_RUN_M: f32 = 0.15;

fn layout(path: &[P2], width: f32) -> Result<Layout, String> {
    if !(3..=4).contains(&path.len()) {
        return Err(
            "a turned flight's `path` needs 3 or 4 points: the bottom centre, one or two corners and the top centre"
                .into(),
        );
    }
    if !path.iter().all(|p| p[0].is_finite() && p[1].is_finite()) {
        return Err("`path` points must be numbers".into());
    }
    if !(width.is_finite() && (0.2..=20.0).contains(&width)) {
        return Err("`width_m` must be within 0.2..=20 m".into());
    }
    let h = width / 2.0;
    let m = path.len();
    let mut d: Vec<P2> = Vec::new();
    let mut len: Vec<f32> = Vec::new();
    for i in 0..m - 1 {
        let v = [path[i + 1][0] - path[i][0], path[i + 1][1] - path[i][1]];
        let l = v[0].hypot(v[1]);
        if l < 1e-3 {
            return Err(format!("`path` points {i} and {} are the same", i + 1));
        }
        let u = [v[0] / l, v[1] / l];
        let dir = if i == 0 {
            u
        } else {
            let p = d[i - 1];
            let dot = u[0] * p[0] + u[1] * p[1];
            if dot.abs() > RIGHT_ANGLE_TOL {
                let angle = (p[0] * u[1] - p[1] * u[0]).abs().atan2(dot).to_degrees();
                return Err(format!(
                    "the path must turn at right angles: the turn at point {i} is {angle:.1} degrees"
                ));
            }
            let n = [-p[1], p[0]];
            let side = if n[0] * u[0] + n[1] * u[1] >= 0.0 {
                1.0
            } else {
                -1.0
            };
            [n[0] * side, n[1] * side]
        };
        len.push(v[0] * dir[0] + v[1] * dir[1]);
        d.push(dir);
    }
    if m == 4 {
        if len[1] < width - 0.01 {
            return Err(format!(
                "the two corners are {:.2} m apart, less than the width {width:.2} m: the flights would overlap; \
                 move them at least one width apart",
                len[1]
            ));
        }
        len[1] = len[1].max(width);
    }
    let mut q = vec![path[0]];
    for i in 0..m - 1 {
        let p = q[i];
        q.push([p[0] + d[i][0] * len[i], p[1] + d[i][1] * len[i]]);
    }
    let at = |p: P2, dir: P2, k: f32| [p[0] + dir[0] * k, p[1] + dir[1] * k];
    let last = m - 2;
    let flights = vec![
        FlightRun {
            start: q[0],
            dir: d[0],
            run: len[0] - h,
        },
        FlightRun {
            start: at(q[last], d[last], h),
            dir: d[last],
            run: len[last] - h,
        },
    ];
    // With two corners the middle segment is the landing, so the last flight starts at the
    // second corner.
    let mut landing = Vec::new();
    let landing_size = if m == 3 {
        landing.push(PlanRect {
            centre: q[1],
            axis: d[0],
            half: [h, h],
        });
        [width, width]
    } else {
        landing.push(PlanRect {
            centre: q[1],
            axis: d[1],
            half: [h, h],
        });
        if len[1] > width + 1e-4 {
            landing.push(PlanRect {
                centre: [(q[1][0] + q[2][0]) / 2.0, (q[1][1] + q[2][1]) / 2.0],
                axis: d[1],
                half: [(len[1] - width) / 2.0, h],
            });
        }
        landing.push(PlanRect {
            centre: q[2],
            axis: d[1],
            half: [h, h],
        });
        [width, len[1] + width]
    };
    for (i, f) in flights.iter().enumerate() {
        if f.run < MIN_RUN_M {
            return Err(format!(
                "flight {} is too short: {:.2} m is left for steps after the landing (needs {MIN_RUN_M} m)",
                i + 1,
                f.run
            ));
        }
    }
    Ok(Layout {
        q,
        d,
        len,
        flights,
        landing,
        landing_size,
        width,
    })
}

impl Layout {
    /// The outline: the union of the flights and the landing, counter-clockwise, without
    /// collinear points. All rectangles are axis-aligned in the frame of the first direction.
    fn outline(&self) -> Result<Vec<P2>, String> {
        let (o, d0) = (self.q[0], self.d[0]);
        let n0 = [-d0[1], d0[0]];
        let local = |p: P2| {
            let v = [p[0] - o[0], p[1] - o[1]];
            [v[0] * d0[0] + v[1] * d0[1], v[0] * n0[0] + v[1] * n0[1]]
        };
        let q = |v: f32| (v * 1e4).round() as i64;
        let mut rects: Vec<[i64; 4]> = Vec::new();
        let hw = self.width / 2.0;
        let flights = self.flights.iter().map(|f| PlanRect {
            centre: [
                f.start[0] + f.dir[0] * f.run / 2.0,
                f.start[1] + f.dir[1] * f.run / 2.0,
            ],
            axis: f.dir,
            half: [f.run / 2.0, hw],
        });
        let flights: Vec<PlanRect> = flights.collect();
        for r in flights.iter().chain(&self.landing) {
            let b = Block {
                centre: r.centre,
                axis: r.axis,
                half: r.half,
                rise: (0.0, 0.0),
                step: false,
            };
            let c = b.corners().map(local);
            let lo = [
                c.iter().map(|p| p[0]).fold(f32::MAX, f32::min),
                c.iter().map(|p| p[1]).fold(f32::MAX, f32::min),
            ];
            let hi = [
                c.iter().map(|p| p[0]).fold(f32::MIN, f32::max),
                c.iter().map(|p| p[1]).fold(f32::MIN, f32::max),
            ];
            rects.push([q(lo[0]), q(lo[1]), q(hi[0]), q(hi[1])]);
        }
        let axis = |k: usize| {
            let mut v: Vec<i64> = rects.iter().flat_map(|r| [r[k], r[k + 2]]).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let (xs, ys) = (axis(0), axis(1));
        let covered = |i: isize, j: isize| {
            i >= 0
                && j >= 0
                && (i as usize) + 1 < xs.len()
                && (j as usize) + 1 < ys.len()
                && rects.iter().any(|r| {
                    r[0] <= xs[i as usize]
                        && xs[i as usize + 1] <= r[2]
                        && r[1] <= ys[j as usize]
                        && ys[j as usize + 1] <= r[3]
                })
        };
        // Boundary edges of the covered cells, counter-clockwise, by start vertex.
        let mut edges: std::collections::BTreeMap<(usize, usize), Vec<(usize, usize)>> =
            Default::default();
        let mut count = 0;
        for i in 0..xs.len().saturating_sub(1) {
            for j in 0..ys.len().saturating_sub(1) {
                if !covered(i as isize, j as isize) {
                    continue;
                }
                let (ii, jj) = (i as isize, j as isize);
                let mut add = |a: (usize, usize), b: (usize, usize)| {
                    edges.entry(a).or_default().push(b);
                    count += 1;
                };
                if !covered(ii, jj - 1) {
                    add((i, j), (i + 1, j));
                }
                if !covered(ii + 1, jj) {
                    add((i + 1, j), (i + 1, j + 1));
                }
                if !covered(ii, jj + 1) {
                    add((i + 1, j + 1), (i, j + 1));
                }
                if !covered(ii - 1, jj) {
                    add((i, j + 1), (i, j));
                }
            }
        }
        let start = *edges.keys().next().ok_or("the flight has no footprint")?;
        let mut ring = vec![start];
        let mut at = start;
        loop {
            let outs = edges.get(&at).ok_or("the footprint is not one outline")?;
            if outs.len() != 1 {
                return Err("the flights touch only at a point".into());
            }
            at = outs[0];
            if at == start {
                break;
            }
            ring.push(at);
            if ring.len() > count {
                return Err("the footprint is not one outline".into());
            }
        }
        if ring.len() != count {
            return Err("the footprint is not one outline (it has a hole)".into());
        }
        // Drop the vertices in the middle of a straight run.
        let n = ring.len();
        let keep: Vec<(usize, usize)> = (0..n)
            .filter(|&k| {
                let (a, b, c) = (ring[(k + n - 1) % n], ring[k], ring[(k + 1) % n]);
                !((a.0 == b.0 && b.0 == c.0) || (a.1 == b.1 && b.1 == c.1))
            })
            .map(|k| ring[k])
            .collect();
        Ok(keep
            .into_iter()
            .map(|(i, j)| {
                let (x, y) = (xs[i] as f32 / 1e4, ys[j] as f32 / 1e4);
                [o[0] + d0[0] * x + n0[0] * y, o[1] + d0[1] * x + n0[1] * y]
            })
            .collect())
    }
}

/// The outline of a flight that turns along `path`, `width` wide: an L of 6 points, a U or S
/// of 8. The reason if the path is not a valid one.
pub fn turned_outline(path: &[P2], width: f32) -> Result<Vec<P2>, String> {
    layout(path, width)?.outline()
}

fn quad(s: &mut dyn Sink, face: usize, v: [[f32; 3]; 4], outward: [f32; 3]) {
    s.tri(face, [v[0], v[1], v[2]], outward);
    s.tri(face, [v[0], v[2], v[3]], outward);
}

/// How low each step's block goes: the lowest ground under the step's footprint (its corners,
/// edge midpoints and a few points between, as [`crate::shape::footing`] samples a footprint)
/// less `lift`, the height the whole flight stands at, but never above `lo`, the bottom of the
/// flight. Without ground every block starts at `lo`.
pub fn step_bottoms(
    g: &Geometry,
    lo: f32,
    lift: f32,
    ground: &dyn Fn(f32, f32) -> Option<f32>,
) -> Vec<f32> {
    if let Some(t) = &g.turned {
        return t
            .blocks
            .iter()
            .map(|b| {
                let n = [-b.axis[1], b.axis[0]];
                let mut low = f32::INFINITY;
                for i in 0..=4 {
                    for j in 0..=4 {
                        let u = b.half[0] * (i as f32 / 2.0 - 1.0);
                        let w = b.half[1] * (j as f32 / 2.0 - 1.0);
                        let (x, z) = (
                            b.centre[0] + b.axis[0] * u + n[0] * w,
                            b.centre[1] + b.axis[1] * u + n[1] * w,
                        );
                        low = low.min(ground(x, z).unwrap_or(0.0));
                    }
                }
                (low - lift).min(lo)
            })
            .collect();
    }
    let d = g.dir;
    let n = [-d[1], d[0]];
    let hw = g.width / 2.0;
    let us = offsets(&g.goings);
    (1..=g.steps)
        .map(|k| {
            let u0 = us[k - 1];
            let u1 = if k == g.steps { g.length } else { us[k] };
            let mut low = f32::INFINITY;
            for i in 0..=4 {
                for j in 0..=4 {
                    let u = u0 + (u1 - u0) * i as f32 / 4.0;
                    let w = -hw + g.width * j as f32 / 4.0;
                    let (x, z) = (
                        g.origin[0] + d[0] * u + n[0] * w,
                        g.origin[1] + d[1] * u + n[1] * w,
                    );
                    low = low.min(ground(x, z).unwrap_or(0.0));
                }
            }
            (low - lift).min(lo)
        })
        .collect()
}

/// Draws the flight `g` from `lo` up on the faces of [`STAIR_FACES`]. `bottoms` holds the bottom
/// of each step's block (see [`step_bottoms`]), all at or under `lo` (a flight set into the slope
/// has them under each step's riser instead), so that on uneven ground every step stays solid
/// down to the ground under it.
pub(crate) fn mesh(s: &mut dyn Sink, g: &Geometry, lo: f32, bottoms: &[f32]) {
    if let Some(t) = &g.turned {
        return mesh_blocks(s, &t.blocks, lo, bottoms);
    }
    let d = g.dir;
    let n = [-d[1], d[0]];
    let hw = g.width / 2.0;
    let p = |u: f32, w: f32, y: f32| {
        [
            g.origin[0] + d[0] * u + n[0] * w,
            y,
            g.origin[1] + d[1] * u + n[1] * w,
        ]
    };
    let (d3, n3, up) = ([d[0], 0.0, d[1]], [n[0], 0.0, n[1]], [0.0, 1.0, 0.0]);
    let neg = |v: [f32; 3]| v.map(|c| -c);
    let hi = lo + g.riser * g.steps as f32;
    let bottom = |k: usize| bottoms.get(k - 1).copied().unwrap_or(lo);
    // A wall across the flight at `u` from `ya` to `yb`, facing `outward`.
    let wall = |s: &mut dyn Sink, face: usize, u: f32, ya: f32, yb: f32, outward: [f32; 3]| {
        quad(
            s,
            face,
            [p(u, -hw, ya), p(u, hw, ya), p(u, hw, yb), p(u, -hw, yb)],
            outward,
        );
    };
    wall(s, SIDES, g.length, bottom(g.steps), hi, d3);
    // What the block before has under the riser: the bottom of the flight for the first step.
    let mut before = lo;
    let us = offsets(&g.goings);
    for k in 1..=g.steps {
        let u0 = us[k - 1];
        let u1 = if k == g.steps { g.length } else { us[k] };
        let (y0, y1) = (lo + (k - 1) as f32 * g.riser, lo + k as f32 * g.riser);
        let yb = bottom(k);
        quad(
            s,
            SIDES,
            [p(u0, -hw, yb), p(u1, -hw, yb), p(u1, hw, yb), p(u0, hw, yb)],
            neg(up),
        );
        quad(
            s,
            TREADS,
            [p(u0, -hw, y1), p(u1, -hw, y1), p(u1, hw, y1), p(u0, hw, y1)],
            up,
        );
        wall(s, RISERS, u0, y0, y1, neg(d3));
        // Where this block goes lower than the one before, its front shows under it; where the
        // one before goes lower, its back shows.
        if yb < before {
            wall(s, SIDES, u0, yb, before, neg(d3));
        } else if yb > before && k > 1 {
            wall(s, SIDES, u0, before, yb, d3);
        }
        before = yb;
        for (w, out) in [(-hw, neg(n3)), (hw, n3)] {
            quad(
                s,
                SIDES,
                [p(u0, w, yb), p(u1, w, yb), p(u1, w, y1), p(u0, w, y1)],
                out,
            );
        }
    }
}

/// One vertical wall of a block: outward normal, the two plan ends, the plane's offset along the
/// normal and whether it is the riser side of a step.
struct Wall {
    n: P2,
    ends: [P2; 2],
    plane: f32,
    front: bool,
}

impl Wall {
    /// The extent of the wall along the direction `t`.
    fn span(&self, t: P2) -> (f32, f32) {
        let a = self.ends[0][0] * t[0] + self.ends[0][1] * t[1];
        let b = self.ends[1][0] * t[0] + self.ends[1][1] * t[1];
        (a.min(b), a.max(b))
    }
}

fn walls(b: &Block) -> [Wall; 4] {
    let (a, n) = (b.axis, [-b.axis[1], b.axis[0]]);
    let [hu, hw] = b.half;
    let p = |u: f32, w: f32| {
        [
            b.centre[0] + a[0] * u + n[0] * w,
            b.centre[1] + a[1] * u + n[1] * w,
        ]
    };
    let wall = |nd: P2, p0: P2, p1: P2, front: bool| Wall {
        n: nd,
        ends: [p0, p1],
        plane: nd[0] * p0[0] + nd[1] * p0[1],
        front,
    };
    [
        wall(a, p(hu, -hw), p(hu, hw), false),
        wall(a.map(|c| -c), p(-hu, hw), p(-hu, -hw), true),
        wall(n, p(hu, hw), p(-hu, hw), false),
        wall(n.map(|c| -c), p(-hu, -hw), p(hu, -hw), false),
    ]
}

/// `a` less `b`, as up to two intervals.
fn minus(a: (f32, f32), b: (f32, f32)) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    if b.1 <= a.0 || b.0 >= a.1 {
        return vec![a];
    }
    if b.0 > a.0 {
        out.push((a.0, b.0));
    }
    if b.1 < a.1 {
        out.push((b.1, a.1));
    }
    out
}

/// Draws a stair that turns: every block is a closed box from its bottom to its tread, and
/// a wall is left out where another block's wall lies right against it (same plane, same
/// extent), less the part of it that block does not cover. Walls that only partly meet (the
/// flanks of the two flights of a U) are drawn whole; they are inside the solid.
fn mesh_blocks(s: &mut dyn Sink, blocks: &[Block], lo: f32, bottoms: &[f32]) {
    let eps = 2e-3;
    let range = |i: usize| {
        let yb = bottoms.get(i).copied().unwrap_or(lo);
        (yb, lo + blocks[i].rise.1)
    };
    let all: Vec<[Wall; 4]> = blocks.iter().map(walls).collect();
    let up = [0.0, 1.0, 0.0];
    for (i, b) in blocks.iter().enumerate() {
        let (yb, y1) = range(i);
        let corners = b.corners();
        let at = |y: f32| corners.map(|c| [c[0], y, c[1]]);
        quad(s, TREADS, at(y1), up);
        quad(s, SIDES, at(yb), [0.0, -1.0, 0.0]);
        let riser_from = lo + b.rise.0;
        for w in &all[i] {
            let t = [-w.n[1], w.n[0]];
            let own = w.span(t);
            let mut open = vec![(yb, y1)];
            for (j, other) in all.iter().enumerate() {
                if j == i {
                    continue;
                }
                for w2 in other {
                    let (s2, r) = (w2.span(t), range(j));
                    if w.n[0] * w2.n[0] + w.n[1] * w2.n[1] < -0.99
                        && (w2.ends[0][0] * w.n[0] + w2.ends[0][1] * w.n[1] - w.plane).abs() < eps
                        && (s2.0 - own.0).abs() < eps
                        && (s2.1 - own.1).abs() < eps
                    {
                        open = open.into_iter().flat_map(|iv| minus(iv, r)).collect();
                    }
                }
            }
            let out = [w.n[0], 0.0, w.n[1]];
            let mut put = |face: usize, a: f32, z: f32| {
                if z - a > 1e-5 {
                    let (p0, p1) = (w.ends[0], w.ends[1]);
                    quad(
                        s,
                        face,
                        [
                            [p0[0], a, p0[1]],
                            [p1[0], a, p1[1]],
                            [p1[0], z, p1[1]],
                            [p0[0], z, p0[1]],
                        ],
                        out,
                    );
                }
            };
            for (a, z) in open {
                if w.front && b.step && z > riser_from {
                    put(SIDES, a, riser_from.max(a).min(z));
                    put(RISERS, a.max(riser_from), z);
                } else {
                    put(SIDES, a, z);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::{Extrusion, Shape, shape_mesh};

    /// A flight of `len` m by `width` m from z = 0 (bottom) up to z = `len`, rising `height`.
    fn flight(len: f32, width: f32, height: f32, stairs: Stairs) -> Shape {
        Shape::Extrusion(Extrusion {
            id: "S1".into(),
            name: "Stairs".into(),
            layer: "shapes".into(),
            // Edge 2 (point 2 to 3) is the top, at z = len.
            points: vec![[0.0, 0.0], [width, 0.0], [width, len], [0.0, len]],
            curves: Default::default(),
            smooth: false,
            closed: true,
            base: 0.0,
            height,
            thickness: 0.2,
            extensions: Default::default(),
            house: None,
            fence: None,
            stairs: Some(stairs),
            raised_bed: None,
            furniture: None,
        })
    }

    fn top_edge() -> Stairs {
        Stairs::new(2)
    }

    #[test]
    fn the_step_count_and_positions_follow_the_rise() {
        // 0.51 m at 0.17 m a riser: 3 steps, each 0.17 high, 0.30 deep with a 0.30 tread.
        let mut st = top_edge();
        st.tread_m = Some(0.30);
        let outline = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.2], [0.0, 1.2]];
        let g = st.geometry(&outline, 0.51).unwrap();
        assert_eq!(g.steps, 3);
        assert!((g.riser - 0.17).abs() < 1e-5 && (g.tread - 0.30).abs() < 1e-5);
        assert!((g.length - 1.2).abs() < 1e-5 && (g.width - 1.0).abs() < 1e-5);
        assert_eq!(g.origin, [0.5, 0.0]);
        assert!(g.dir[1] > 0.99);
        assert!(g.warnings.is_empty(), "{:?}", g.warnings);
        // Rounds: 0.5 m is still 3 steps, 0.6 m is 4 (0.15 each).
        assert_eq!(st.geometry(&outline, 0.5).unwrap().steps, 3);
        let g4 = st.geometry(&outline, 0.6).unwrap();
        assert_eq!(g4.steps, 4);
        assert!((g4.riser - 0.15).abs() < 1e-5);
    }

    #[test]
    fn treads_sit_at_every_riser_height_and_the_footprint_is_filled() {
        let shape = flight(
            1.2,
            1.0,
            0.51,
            Stairs {
                tread_m: Some(0.30),
                ..top_edge()
            },
        );
        let m = shape_mesh(&shape).unwrap();
        let treads = m.faces.iter().position(|f| f.name == "S1:treads").unwrap() as u32;
        let mut heights: Vec<f32> = Vec::new();
        for (t, f) in m.triangles.iter().zip(&m.triangle_faces) {
            if *f == treads {
                let y = m.positions[t[0] as usize][1];
                if !heights.iter().any(|h| (h - y).abs() < 1e-4) {
                    heights.push(y);
                }
            }
        }
        heights.sort_by(f32::total_cmp);
        let want = [0.17, 0.34, 0.51];
        assert_eq!(heights.len(), 3, "{heights:?}");
        for (h, w) in heights.iter().zip(want) {
            assert!((h - w).abs() < 1e-4, "{heights:?}");
        }
        // The treads add up to the footprint's area (1.0 x 1.2).
        let area: f32 = m
            .triangles
            .iter()
            .zip(&m.triangle_faces)
            .filter(|(_, f)| **f == treads)
            .map(|(t, _)| {
                let [a, b, c] = t.map(|i| m.positions[i as usize]);
                ((b[0] - a[0]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[0] - a[0])).abs() / 2.0
            })
            .sum();
        assert!((area - 1.2).abs() < 1e-3, "{area}");
    }

    #[test]
    fn the_mesh_is_finite_named_bounded_and_closed() {
        for up_edge in 0..4 {
            let mut shape = flight(1.5, 0.9, 0.6, Stairs::new(up_edge));
            if let Shape::Extrusion(e) = &mut shape {
                e.base = 0.2;
            }
            let m = shape_mesh(&shape).unwrap();
            let names: Vec<&str> = m.faces.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(names, ["S1:treads", "S1:risers", "S1:sides"]);
            for i in 0..3u32 {
                assert!(m.triangle_faces.contains(&i), "face {i} is empty");
            }
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &m.positions {
                for k in 0..3 {
                    assert!(p[k].is_finite());
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            assert!(
                (lo[1] - 0.2).abs() < 1e-5 && (hi[1] - 0.8).abs() < 1e-5,
                "{lo:?} {hi:?}"
            );
            assert!(
                lo[0].abs() < 1e-5 && (hi[0] - 0.9).abs() < 1e-5,
                "{lo:?} {hi:?}"
            );
            assert!(
                lo[2].abs() < 1e-5 && (hi[2] - 1.5).abs() < 1e-5,
                "{lo:?} {hi:?}"
            );
        }
    }

    #[test]
    fn the_volume_is_a_flight_of_blocks() {
        let shape = flight(
            1.2,
            1.0,
            0.51,
            Stairs {
                tread_m: Some(0.30),
                ..top_edge()
            },
        );
        let m = shape_mesh(&shape).unwrap();
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
        // 3 steps 0.3 deep (the top one runs to the back: 0.6 deep), 1 m wide.
        let want = 1.0 * (0.17 * 0.3 + 0.34 * 0.3 + 0.51 * 0.6);
        assert!((v - want).abs() < 1e-3, "{v} vs {want}");
    }

    #[test]
    fn blondel_and_short_footprints_warn() {
        let outline = [[0.0, 0.0], [1.0, 0.0], [1.0, 3.0], [0.0, 3.0]];
        // Steeper than comfortable: 2 x 0.25 + 0.30 = 0.80.
        let steep = Stairs {
            up_edge: 2,
            riser_m: Some(0.25),
            tread_m: Some(0.30),
            ..Default::default()
        };
        let g = steep.geometry(&outline, 0.75).unwrap();
        assert!(
            g.warnings.iter().any(|w| w.contains("Blondel")),
            "{:?}",
            g.warnings
        );
        // Too short for the treads: 6 steps of 0.30 m do not fit 1 m.
        let short = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let g = Stairs {
            tread_m: Some(0.30),
            ..top_edge()
        }
        .geometry(&short, 1.0)
        .unwrap();
        assert!(
            g.warnings.iter().any(|w| w.contains("too short")),
            "{:?}",
            g.warnings
        );
        assert!((g.tread - 1.0 / 6.0).abs() < 1e-5);
        // The warnings reach the mesh.
        let m = shape_mesh(&flight(
            1.0,
            1.0,
            1.0,
            Stairs {
                tread_m: Some(0.30),
                ..top_edge()
            },
        ))
        .unwrap();
        assert!(m.warnings.iter().any(|w| w.contains("too short")));
        // A comfortable flight has none.
        let g = Stairs::new(2).geometry(&[[0.0, 0.0], [1.0, 0.0], [1.0, 0.9], [0.0, 0.9]], 0.51);
        assert!(g.unwrap().warnings.is_empty());
    }

    #[test]
    fn bad_stairs_are_refused_with_a_reason() {
        let rect = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let st = Stairs::new(2);
        assert!(st.geometry(&rect[..3], 0.5).is_err());
        assert!(Stairs::new(4).geometry(&rect, 0.5).is_err());
        assert!(st.geometry(&rect, f32::NAN).is_err());
        assert!(st.geometry(&rect, 0.0).is_err());
        let skew = [[0.0, 0.0], [1.0, 0.0], [1.4, 1.0], [0.0, 1.0]];
        assert!(st.geometry(&skew, 0.5).unwrap_err().contains("rectangle"));
        let bad = Stairs {
            riser_m: Some(0.9),
            ..Stairs::new(2)
        };
        assert!(bad.geometry(&rect, 0.5).unwrap_err().contains("riser_m"));
        // 18 steps in 1 m: no tread left.
        assert!(st.geometry(&rect, 3.0).unwrap_err().contains("too short"));
    }

    fn volume(m: &crate::ImportedModel) -> f32 {
        m.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| m.positions[i as usize]);
                (a[0] * (b[1] * c[2] - b[2] * c[1])
                    + a[1] * (b[2] * c[0] - b[0] * c[2])
                    + a[2] * (b[0] * c[1] - b[1] * c[0]))
                    / 6.0
            })
            .sum::<f32>()
            .abs()
    }

    /// The volume the closed mesh of `g` (from 0) encloses, each triangle turned to face its
    /// stated outward side: a mesh that is not closed or has a wrong side gives another number.
    fn mesh_volume(g: &Geometry, bottoms: &[f32]) -> f32 {
        struct Vol(f32);
        impl Sink for Vol {
            fn tri(&mut self, _: usize, [a, b, c]: [[f32; 3]; 3], outward: [f32; 3]) {
                let det = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| {
                    a[0] * (b[1] * c[2] - b[2] * c[1])
                        + a[1] * (b[2] * c[0] - b[0] * c[2])
                        + a[2] * (b[0] * c[1] - b[1] * c[0])
                };
                let n = [
                    (b[1] - a[1]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[1] - a[1]),
                    (b[2] - a[2]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[2] - a[2]),
                    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]),
                ];
                let flip = n.iter().zip(outward).map(|(x, y)| x * y).sum::<f32>() < 0.0;
                let v = if flip { det(a, c, b) } else { det(a, b, c) };
                self.0 += v / 6.0;
            }
        }
        let mut v = Vol(0.0);
        mesh(&mut v, g, 0.0, bottoms);
        v.0
    }

    #[test]
    fn on_sloping_ground_every_block_goes_down_to_the_ground_under_it() {
        let st = Stairs {
            tread_m: Some(0.30),
            ..top_edge()
        };
        let outline = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.2], [0.0, 1.2]];
        let g = st.geometry(&outline, 0.51).unwrap();
        // The ground falls away 0.2 m per m along the flight, the way a bank runs down past
        // the flight's own footing: the flight stands at 0.
        let fall = |_: f32, z: f32| Some(-0.2 * z);
        let bottoms = step_bottoms(&g, 0.0, 0.0, &fall);
        assert_eq!(bottoms.len(), 3);
        assert!(bottoms.windows(2).all(|w| w[1] < w[0]), "{bottoms:?}");
        // Step 1 covers z 0..0.3 (lowest ground -0.06), step 3 z 0.6..1.2 (-0.24).
        assert!((bottoms[0] + 0.06).abs() < 1e-5 && (bottoms[2] + 0.24).abs() < 1e-5);
        // Level ground, or ground above the flight, changes nothing.
        let level = step_bottoms(&g, 0.0, 0.0, &|_, _| Some(0.0));
        assert_eq!(level, [0.0; 3]);
        assert_eq!(step_bottoms(&g, 0.0, 0.0, &|_, z| Some(z)), [0.0; 3]);
        // The mesh stays closed: its volume is the flat flight's plus the columns under it.
        let v0 = mesh_volume(&g, &[]);
        let extra: f32 = (0..3)
            .map(|k| {
                let u1 = if k == 2 {
                    g.length
                } else {
                    (k + 1) as f32 * g.tread
                };
                1.0 * (u1 - k as f32 * g.tread) * -bottoms[k]
            })
            .sum();
        assert!((mesh_volume(&g, &bottoms) - v0 - extra).abs() < 1e-3);
    }

    #[test]
    fn a_flight_on_a_terrain_with_a_dip_reaches_down_into_it() {
        let mut t = crate::Terrain::flat([-1.0, -1.0], 4.0, 4.0, 0.5, "lawn").unwrap();
        let w = t.width as usize + 1;
        // A trench across the flight at z = 0.5, and the flight's corners and edge midpoints
        // (where its footing is taken) on the flat.
        for (i, h) in t.heights.iter_mut().enumerate() {
            let z = -1.0 + (i / w) as f32 * 0.5;
            if (z - 0.5).abs() < 1e-3 {
                *h = -0.3;
            }
        }
        let shape = flight(
            1.2,
            1.0,
            0.51,
            Stairs {
                tread_m: Some(0.30),
                ..top_edge()
            },
        );
        let flat = shape_mesh(&shape).unwrap();
        let on = crate::shape::shape_mesh_on_terrain(&shape, &t, &[]).unwrap();
        let low =
            |m: &crate::ImportedModel| m.positions.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        assert!(low(&flat).abs() < 1e-6);
        assert!(low(&on) < -0.03, "the flight stops at {}", low(&on));
        assert!(volume(&on) > volume(&flat));
    }

    // --- Flights that turn --------------------------------------------------------------

    fn turned_stairs(path: &[P2], width: f32) -> Stairs {
        Stairs {
            path: Some(path.to_vec()),
            width_m: Some(width),
            ..Default::default()
        }
    }

    fn turned_shape(path: &[P2], width: f32, height: f32, stairs: Stairs) -> Shape {
        Shape::Extrusion(Extrusion {
            id: "S1".into(),
            name: "Stairs".into(),
            layer: "shapes".into(),
            points: turned_outline(path, width).unwrap(),
            curves: Default::default(),
            smooth: false,
            closed: true,
            base: 0.0,
            height,
            thickness: 0.2,
            extensions: Default::default(),
            house: None,
            fence: None,
            stairs: Some(stairs),
            raised_bed: None,
            furniture: None,
        })
    }

    const L_PATH: [P2; 3] = [[0.0, 0.0], [0.0, 2.5], [2.5, 2.5]];
    const U_PATH: [P2; 4] = [[0.0, 0.0], [0.0, 3.0], [1.0, 3.0], [1.0, 0.0]];

    fn area(poly: &[P2]) -> f32 {
        let n = poly.len();
        (0..n)
            .map(|i| {
                let (a, b) = (poly[i], poly[(i + 1) % n]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum::<f32>()
            / 2.0
    }

    /// The signed volume of the closed mesh and the sum of its area vectors (zero when closed).
    fn volume_and_closure(m: &crate::ImportedModel) -> (f32, [f32; 3]) {
        let mut v = 0.0;
        let mut n = [0.0f32; 3];
        for t in &m.triangles {
            let [a, b, c] = t.map(|i| m.positions[i as usize]);
            v += (a[0] * (b[1] * c[2] - b[2] * c[1])
                + a[1] * (b[2] * c[0] - b[0] * c[2])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0;
            let (u, w) = (
                [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
            );
            n[0] += (u[1] * w[2] - u[2] * w[1]) / 2.0;
            n[1] += (u[2] * w[0] - u[0] * w[2]) / 2.0;
            n[2] += (u[0] * w[1] - u[1] * w[0]) / 2.0;
        }
        (v, n)
    }

    fn block_volume(g: &Geometry) -> f32 {
        g.turned
            .as_ref()
            .unwrap()
            .blocks
            .iter()
            .map(|b| 4.0 * b.half[0] * b.half[1] * b.rise.1)
            .sum()
    }

    #[test]
    fn an_l_stair_has_a_six_point_outline_and_shares_the_steps_by_run() {
        let st = turned_stairs(&L_PATH, 1.0);
        let outline = turned_outline(&L_PATH, 1.0).unwrap();
        assert_eq!(outline.len(), 6, "{outline:?}");
        assert!(area(&outline) > 0.0, "counter-clockwise");
        // 1.5 m at 0.17 m: 9 steps; the runs are 2.0 m each, so 5 and 4 (or 4 and 5).
        let g = st.geometry(&outline, 1.5).unwrap();
        assert_eq!(g.steps, 9);
        let t = g.turned.as_ref().unwrap();
        assert_eq!(t.flights.len(), 2);
        assert_eq!(t.flights.iter().map(|f| f.steps).sum::<usize>(), 9);
        assert!(t.flights.iter().all(|f| (f.run_m - 2.0).abs() < 1e-4));
        assert!(t.flights[0].steps.abs_diff(t.flights[1].steps) <= 1);
        assert_eq!(t.landings.len(), 1);
        assert_eq!(t.landings[0].size_m, [1.0, 1.0]);
        // Steps in proportion to the run: a longer second flight gets more.
        let long = [[0.0, 0.0], [0.0, 1.5], [4.5, 1.5]];
        let o = turned_outline(&long, 1.0).unwrap();
        let g = turned_stairs(&long, 1.0).geometry(&o, 1.5).unwrap();
        let f = &g.turned.as_ref().unwrap().flights;
        assert_eq!((f[0].steps, f[1].steps), (2, 7), "{f:?}");
    }

    #[test]
    fn landings_lie_at_the_height_of_the_step_reaching_them() {
        for (path, width) in [(L_PATH.to_vec(), 1.0), (U_PATH.to_vec(), 1.0)] {
            let st = turned_stairs(&path, width);
            let shape = turned_shape(&path, width, 1.5, st.clone());
            let g = st.geometry(shape.points(), 1.5).unwrap();
            let t = g.turned.as_ref().unwrap();
            let n1 = t.flights[0].steps;
            assert!((t.landings[0].height_m - n1 as f32 * g.riser).abs() < 1e-5);
            for b in t.blocks.iter().filter(|b| !b.step) {
                assert!((b.rise.1 - n1 as f32 * g.riser).abs() < 1e-5);
            }
            // The first flight's top step is level with the landing, the second flight's first
            // step one riser above it, the last one at the full height.
            let steps: Vec<&Block> = t.blocks.iter().filter(|b| b.step).collect();
            assert!((steps[n1 - 1].rise.1 - t.landings[0].height_m).abs() < 1e-5);
            assert!((steps[n1].rise.1 - t.landings[0].height_m - g.riser).abs() < 1e-5);
            assert!((steps.last().unwrap().rise.1 - 1.5).abs() < 1e-4);
            // The treads of the mesh sit at the step heights.
            let m = shape_mesh(&shape).unwrap();
            let treads = m.faces.iter().position(|f| f.name == "S1:treads").unwrap() as u32;
            let mut levels: Vec<f32> = Vec::new();
            for (tri, f) in m.triangles.iter().zip(&m.triangle_faces) {
                let y = m.positions[tri[0] as usize][1];
                if *f == treads && !levels.iter().any(|h| (h - y).abs() < 1e-4) {
                    levels.push(y);
                }
            }
            assert_eq!(levels.len(), g.steps, "{levels:?}");
        }
    }

    #[test]
    fn l_and_u_meshes_are_closed_with_every_step_counted() {
        for (path, steps) in [(L_PATH.to_vec(), 9), (U_PATH.to_vec(), 9)] {
            let st = turned_stairs(&path, 1.0);
            let shape = turned_shape(&path, 1.0, 1.5, st.clone());
            let m = shape_mesh(&shape).unwrap();
            let names: Vec<&str> = m.faces.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(names, ["S1:treads", "S1:risers", "S1:sides"]);
            for i in 0..3u32 {
                assert!(m.triangle_faces.contains(&i), "face {i} is empty");
            }
            let g = st.geometry(shape.points(), 1.5).unwrap();
            assert_eq!(g.steps, steps);
            assert_eq!(
                g.turned
                    .as_ref()
                    .unwrap()
                    .flights
                    .iter()
                    .map(|f| f.steps)
                    .sum::<usize>(),
                steps
            );
            let (v, n) = volume_and_closure(&m);
            assert!(n.iter().all(|c| c.abs() < 1e-3), "not closed: {n:?}");
            assert!(
                (v.abs() - block_volume(&g)).abs() < 1e-2,
                "{v} vs {}",
                block_volume(&g)
            );
            assert!(m.positions.iter().all(|p| p.iter().all(|c| c.is_finite())));
        }
    }

    fn overlap(a: &Block, b: &Block) -> bool {
        let (ca, cb) = (a.corners(), b.corners());
        // Separating axes: both blocks' edges. Touching is not overlapping.
        [
            a.axis,
            [-a.axis[1], a.axis[0]],
            b.axis,
            [-b.axis[1], b.axis[0]],
        ]
        .iter()
        .all(|ax| {
            let proj = |c: &[P2; 4]| {
                let v: Vec<f32> = c.iter().map(|p| p[0] * ax[0] + p[1] * ax[1]).collect();
                (
                    v.iter().copied().fold(f32::MAX, f32::min),
                    v.iter().copied().fold(f32::MIN, f32::max),
                )
            };
            let (pa, pb) = (proj(&ca), proj(&cb));
            pa.1 > pb.0 + 1e-3 && pb.1 > pa.0 + 1e-3
        })
    }

    #[test]
    fn no_block_of_a_u_stair_overlaps_another() {
        for path in [
            U_PATH.to_vec(),
            // A wide landing with a well between the flights.
            vec![[0.0, 0.0], [0.0, 3.0], [2.0, 3.0], [2.0, 0.0]],
            // An S stair: the second turn goes the other way.
            vec![[0.0, 0.0], [0.0, 3.0], [1.5, 3.0], [1.5, 6.0]],
            L_PATH.to_vec(),
        ] {
            let st = turned_stairs(&path, 1.0);
            let o = turned_outline(&path, 1.0).unwrap();
            let g = st.geometry(&o, 2.0).unwrap();
            let blocks = &g.turned.as_ref().unwrap().blocks;
            for (i, a) in blocks.iter().enumerate() {
                for b in &blocks[i + 1..] {
                    assert!(!overlap(a, b), "{path:?}: {a:?} and {b:?}");
                }
            }
        }
    }

    #[test]
    fn the_outline_is_the_area_of_the_flights_and_landings() {
        for (path, width, outline_len) in [
            (L_PATH.to_vec(), 1.0, 6),
            // A half-landing: the two flights and the landing fill one rectangle.
            (U_PATH.to_vec(), 1.0, 4),
            // A well between the flights: a U of 8 points.
            (vec![[0.0, 0.0], [0.0, 3.0], [2.0, 3.0], [2.0, 0.0]], 1.0, 8),
            (L_PATH.to_vec(), 1.4, 6),
        ] {
            let o = turned_outline(&path, width).unwrap();
            assert_eq!(o.len(), outline_len, "{path:?}: {o:?}");
            assert!(area(&o) > 0.0, "counter-clockwise");
            let st = turned_stairs(&path, width);
            let g = st.geometry(&o, 1.5).unwrap();
            let t = g.turned.as_ref().unwrap();
            let sum: f32 = t.flights.iter().map(|f| f.run_m * width).sum::<f32>()
                + t.landings
                    .iter()
                    .map(|l| l.size_m[0] * l.size_m[1])
                    .sum::<f32>();
            assert!((area(&o) - sum).abs() < 1e-3, "{} vs {sum}", area(&o));
        }
    }

    #[test]
    fn bad_paths_are_refused_with_a_reason() {
        let err = |path: &[P2], width: f32| turned_outline(path, width).unwrap_err();
        // A corner at 80 degrees.
        let e = err(&[[0.0, 0.0], [0.0, 2.5], [2.5, 3.0]], 1.0);
        assert!(e.contains("right angle"), "{e}");
        // A path that doubles back.
        assert!(err(&[[0.0, 0.0], [0.0, 2.5], [0.0, 0.0]], 1.0).contains("right angle"));
        // Within a degree is accepted (and snapped).
        assert!(turned_outline(&[[0.0, 0.0], [0.0, 2.5], [2.5, 2.54]], 1.0).is_ok());
        // Corners closer than a width would make the flights overlap.
        let e = err(&[[0.0, 0.0], [0.0, 3.0], [0.6, 3.0], [0.6, 0.0]], 1.0);
        assert!(e.contains("overlap"), "{e}");
        // Too few or too many points, a run too short for a step.
        assert!(err(&[[0.0, 0.0], [0.0, 2.0]], 1.0).contains("3 or 4"));
        assert!(err(&[[0.0, 0.0], [0.0, 0.5], [2.5, 0.5]], 1.0).contains("too short"));
        // A path without a width, or with an outline that is not its own.
        let o = turned_outline(&L_PATH, 1.0).unwrap();
        let no_width = Stairs {
            width_m: None,
            ..turned_stairs(&L_PATH, 1.0)
        };
        assert!(no_width.geometry(&o, 1.5).unwrap_err().contains("width_m"));
        let st = turned_stairs(&L_PATH, 1.0);
        let moved: Vec<P2> = o.iter().map(|p| [p[0] + 0.5, p[1]]).collect();
        assert!(st.geometry(&moved, 1.5).unwrap_err().contains("path"));
        // Too few steps for the flights.
        let few = Stairs {
            riser_m: Some(0.3),
            ..turned_stairs(&L_PATH, 1.0)
        };
        assert!(few.geometry(&o, 0.3).unwrap_err().contains("too few"));
    }

    #[test]
    fn a_turned_flight_warns_per_flight_and_its_blocks_reach_the_ground() {
        let path = [[0.0, 0.0], [0.0, 2.5], [2.5, 2.5]];
        let st = Stairs {
            tread_m: Some(0.30),
            ..turned_stairs(&path, 1.0)
        };
        let o = turned_outline(&path, 1.0).unwrap();
        // 3 m of rise: 18 steps, 9 a flight: 2.7 m of treads in 2 m runs.
        let g = st.geometry(&o, 3.0).unwrap();
        assert!(
            g.warnings
                .iter()
                .any(|w| w.contains("flight 1") && w.contains("too short")),
            "{:?}",
            g.warnings
        );
        // On sloping ground each block goes down to the ground under it.
        let g = st.geometry(&o, 1.0).unwrap();
        let fall = |_: f32, z: f32| Some(-0.2 * z);
        let bottoms = step_bottoms(&g, 0.0, 0.0, &fall);
        let t = g.turned.as_ref().unwrap();
        assert_eq!(bottoms.len(), t.blocks.len());
        assert!(bottoms.iter().all(|b| *b <= 0.0) && bottoms.iter().any(|b| *b < -0.1));
    }
}
