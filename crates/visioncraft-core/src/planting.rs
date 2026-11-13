//! Plants as editable generators, not baked instances: the project stores the recipe, [`evaluate`]
//! turns it into placements, deterministically (a small SplitMix64 of our own, no `rand`).

use crate::ImportedModel;
use crate::material::Lawn3d;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Hard cap on the items one generator evaluates to, so a typo in a density cannot freeze a browser.
pub const MAX_PLACEMENTS: usize = 200_000;
/// Hard cap on grid cells visited by one scatter.
const MAX_CELLS: usize = 4_000_000;

/// An evaluated instance of a catalogue asset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub asset: String,
    pub position: [f32; 3],
    pub rotation_y_deg: f32,
    pub scale: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Weighted {
    pub asset: String,
    pub weight: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Spacing {
    /// This many items, first and last at the ends of an open line.
    Count(u32),
    /// One item every this many metres.
    Every(f32),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Single {
    pub id: String,
    pub name: String,
    pub layer: String,
    pub asset: String,
    pub position: [f32; 3],
    pub rotation_y_deg: f32,
    pub scale: f32,
}

/// Items along a Catmull-Rom curve through `points` (x, z), distributed by arc length.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplineMass {
    pub id: String,
    pub name: String,
    pub layer: String,
    pub asset_palette: Vec<Weighted>,
    pub points: Vec<[f32; 2]>,
    pub closed: bool,
    pub spacing: Spacing,
    /// 0..1: how far an item may slide along the line, as a fraction of the spacing.
    pub spacing_jitter: f32,
    /// Sideways shift in metres, positive to the right of the direction of travel seen from above.
    pub offset_m: f32,
    pub scale_min: f32,
    pub scale_max: f32,
    pub random_rotation: bool,
    pub seed: u64,
}

/// Points inside a polygon on a jittered grid anchored at the world origin: a point depends only
/// on its own cell, so editing the outline far away leaves it where it was.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AreaScatter {
    pub id: String,
    pub name: String,
    pub layer: String,
    pub asset_palette: Vec<Weighted>,
    pub outline: Vec<[f32; 2]>,
    pub density_per_m2: f32,
    /// 0..1: how far a point may stray from its cell centre, as a fraction of the cell.
    pub position_jitter: f32,
    pub scale_min: f32,
    pub scale_max: f32,
    pub random_rotation: bool,
    pub min_distance_m: f32,
    pub seed: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub points: Vec<[f32; 2]>,
    pub radius_m: f32,
    /// Removes coverage instead of adding it.
    pub erase: bool,
}

/// Plants painted with a brush: strokes add and erase coverage in order; rotation is always random.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MixedBrush {
    pub id: String,
    pub name: String,
    pub layer: String,
    pub asset_palette: Vec<Weighted>,
    pub strokes: Vec<Stroke>,
    pub density_per_m2: f32,
    pub scale_min: f32,
    pub scale_max: f32,
    pub seed: u64,
}

/// Fills named faces of a model or shape with plants; needs the owner's mesh, see
/// [`evaluate_surface_fill`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceFill {
    pub id: String,
    pub name: String,
    pub layer: String,
    pub asset_palette: Vec<Weighted>,
    /// A model or shape id.
    pub owner: String,
    /// Persistent face names on that owner.
    pub faces: Vec<String>,
    pub density_per_m2: f32,
    /// Accepted for the shared conventions; surface points are uniformly random, so it has no effect.
    pub position_jitter: f32,
    pub scale_min: f32,
    pub scale_max: f32,
    pub random_rotation: bool,
    pub min_distance_m: f32,
    /// Triangles steeper than this are not planted.
    #[serde(default = "default_slope")]
    pub max_slope_deg: f32,
    pub seed: u64,
}

fn default_slope() -> f32 {
    45.0
}

/// A clump of `count` items in a disc, denser towards the centre.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cluster {
    pub id: String,
    pub name: String,
    pub layer: String,
    pub asset_palette: Vec<Weighted>,
    pub center: [f32; 2],
    pub radius_m: f32,
    pub count: u32,
    pub scale_min: f32,
    pub scale_max: f32,
    pub random_rotation: bool,
    pub seed: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Generator {
    Single(Single),
    SplineMass(SplineMass),
    AreaScatter(AreaScatter),
    MixedBrush(MixedBrush),
    SurfaceFill(SurfaceFill),
    Cluster(Cluster),
}

macro_rules! each {
    ($g:expr, $x:ident => $e:expr) => {
        match $g {
            Generator::Single($x) => $e,
            Generator::SplineMass($x) => $e,
            Generator::AreaScatter($x) => $e,
            Generator::MixedBrush($x) => $e,
            Generator::SurfaceFill($x) => $e,
            Generator::Cluster($x) => $e,
        }
    };
}

impl Generator {
    pub fn id(&self) -> &str {
        each!(self, g => &g.id)
    }

    pub fn name(&self) -> &str {
        each!(self, g => &g.name)
    }

    pub fn layer(&self) -> &str {
        each!(self, g => &g.layer)
    }

    pub(crate) fn layer_mut(&mut self) -> &mut String {
        each!(self, g => &mut g.layer)
    }

    /// Rejects what cannot be evaluated: empty palettes, non-positive densities and so on. Asset
    /// ids are not checked here (the catalogue is not part of the project).
    pub fn validate(&self) -> Result<(), String> {
        let ok = |c: bool, why: &str| if c { Ok(()) } else { Err(why.to_string()) };
        let finite = |v: &[f32]| v.iter().all(|c| c.is_finite());
        let palette = |p: &[Weighted]| {
            ok(
                p.iter().all(|w| w.weight.is_finite() && w.weight >= 0.0)
                    && p.iter().any(|w| w.weight > 0.0),
                "the palette needs at least one asset with weight above zero",
            )
        };
        let scales = |lo: f32, hi: f32| {
            ok(
                lo > 0.0 && lo <= hi && hi.is_finite(),
                "scale needs 0 < min <= max",
            )
        };
        let density = |d: f32| ok(d.is_finite() && d > 0.0, "density must be above zero");
        let unit = |v: f32| ok((0.0..=1.0).contains(&v), "jitter must lie in 0..1");
        let flat = |p: &[[f32; 2]], n: usize| {
            ok(
                p.len() >= n && p.iter().flatten().all(|c| c.is_finite()),
                "too few points, or not finite",
            )
        };
        match self {
            Self::Single(g) => ok(
                finite(&g.position)
                    && g.rotation_y_deg.is_finite()
                    && g.scale.is_finite()
                    && g.scale > 0.0,
                "position, rotation and scale must be finite, scale above zero",
            ),
            Self::SplineMass(g) => {
                palette(&g.asset_palette)?;
                flat(&g.points, 2)?;
                scales(g.scale_min, g.scale_max)?;
                unit(g.spacing_jitter)?;
                ok(g.offset_m.is_finite(), "offset must be finite")?;
                match g.spacing {
                    Spacing::Count(n) => ok(n >= 1, "count must be at least 1"),
                    Spacing::Every(d) => ok(d.is_finite() && d > 0.0, "spacing must be above zero"),
                }
            }
            Self::AreaScatter(g) => {
                palette(&g.asset_palette)?;
                flat(&g.outline, 3)?;
                scales(g.scale_min, g.scale_max)?;
                density(g.density_per_m2)?;
                unit(g.position_jitter)?;
                ok(
                    g.min_distance_m.is_finite() && g.min_distance_m >= 0.0,
                    "min distance must be 0 or more",
                )
            }
            Self::MixedBrush(g) => {
                palette(&g.asset_palette)?;
                scales(g.scale_min, g.scale_max)?;
                density(g.density_per_m2)?;
                g.strokes.iter().try_for_each(|s| {
                    flat(&s.points, 1)?;
                    ok(
                        s.radius_m.is_finite() && s.radius_m > 0.0,
                        "stroke radius must be above zero",
                    )
                })
            }
            Self::SurfaceFill(g) => {
                palette(&g.asset_palette)?;
                scales(g.scale_min, g.scale_max)?;
                density(g.density_per_m2)?;
                ok(
                    g.min_distance_m.is_finite()
                        && g.min_distance_m >= 0.0
                        && (0.0..=90.0).contains(&g.max_slope_deg),
                    "min distance must be 0 or more and the slope 0..90 degrees",
                )
            }
            Self::Cluster(g) => {
                palette(&g.asset_palette)?;
                scales(g.scale_min, g.scale_max)?;
                ok(
                    g.radius_m.is_finite() && g.radius_m > 0.0 && finite(&g.center),
                    "radius must be above zero",
                )
            }
        }
    }
}

// ---- random numbers ----

/// SplitMix64.
#[derive(Clone, Copy)]
pub(crate) struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..1`.
    fn f32(&mut self) -> f32 {
        (self.next() >> 40) as f32 / 16_777_216.0
    }
}

/// A seed for a sub-stream, e.g. one grid cell.
fn mix(a: u64, b: i64) -> u64 {
    Rng::new(a ^ (b as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93)).next()
}

// ---- shared pieces ----

#[derive(Clone, Debug, PartialEq)]
pub struct Evaluation {
    pub placements: Vec<Placement>,
    /// The generator hit [`MAX_PLACEMENTS`] (or the cell limit) and stopped early.
    pub truncated: bool,
}

fn pick<'a>(palette: &'a [Weighted], rng: &mut Rng) -> &'a str {
    let total: f32 = palette.iter().map(|w| w.weight).sum();
    let mut t = rng.f32() * total;
    for w in palette {
        if t < w.weight {
            return &w.asset;
        }
        t -= w.weight;
    }
    // Rounding at the very end: the last asset with weight.
    palette
        .iter()
        .rfind(|w| w.weight > 0.0)
        .map_or("", |w| w.asset.as_str())
}

/// Species, then scale, then rotation, in this order, from `rng`.
fn make(
    palette: &[Weighted],
    (lo, hi): (f32, f32),
    spin: bool,
    position: [f32; 3],
    rng: &mut Rng,
) -> Placement {
    let asset = pick(palette, rng).to_string();
    let scale = lo + (hi - lo) * rng.f32();
    let rotation_y_deg = if spin { 360.0 * rng.f32() } else { 0.0 };
    Placement {
        asset,
        position,
        rotation_y_deg,
        scale,
    }
}

/// Accepts points no closer than `min` to an earlier one. Iteration never depends on map order.
struct Spaced {
    min: f32,
    cells: BTreeMap<(i32, i32, i32), Vec<[f32; 3]>>,
}

impl Spaced {
    fn new(min: f32) -> Self {
        Self {
            min,
            cells: BTreeMap::new(),
        }
    }

    fn try_add(&mut self, p: [f32; 3]) -> bool {
        if self.min <= 0.0 {
            return true;
        }
        let c = p.map(|v| (v / self.min).floor() as i32);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(near) = self.cells.get(&(c[0] + dx, c[1] + dy, c[2] + dz)) else {
                        continue;
                    };
                    let far = |q: &[f32; 3]| {
                        (0..3).map(|k| (q[k] - p[k]).powi(2)).sum::<f32>() >= self.min * self.min
                    };
                    if !near.iter().all(far) {
                        return false;
                    }
                }
            }
        }
        self.cells.entry((c[0], c[1], c[2])).or_default().push(p);
        true
    }
}

fn point_in_polygon(p: [f32; 2], poly: &[[f32; 2]]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn bounds(points: impl Iterator<Item = [f32; 2]>) -> [[f32; 2]; 2] {
    let mut b = [[f32::INFINITY; 2], [f32::NEG_INFINITY; 2]];
    for p in points {
        for k in 0..2 {
            b[0][k] = b[0][k].min(p[k]);
            b[1][k] = b[1][k].max(p[k]);
        }
    }
    b
}

/// One candidate per cell of a grid anchored at the origin with `density` cells per m2; each
/// candidate's position and random stream depend on its cell and `seed` only. Returns the
/// accepted ones in cell order and whether a limit stopped the walk.
fn grid_points(
    bounds: [[f32; 2]; 2],
    density: f32,
    seed: u64,
    jitter: f32,
    min_distance: f32,
    inside: impl Fn([f32; 2]) -> bool,
) -> (Vec<([f32; 2], Rng)>, bool) {
    let cell = 1.0 / density.sqrt();
    let range = |k: usize| {
        (
            (bounds[0][k] / cell).floor() as i64,
            (bounds[1][k] / cell).floor() as i64,
        )
    };
    let ((i0, i1), (j0, j1)) = (range(0), range(1));
    let mut spaced = Spaced::new(min_distance);
    let (mut out, mut visited) = (Vec::new(), 0usize);
    for i in i0..=i1 {
        for j in j0..=j1 {
            visited += 1;
            if visited > MAX_CELLS || out.len() >= MAX_PLACEMENTS {
                return (out, true);
            }
            let mut rng = Rng::new(mix(mix(seed, i), j));
            let p = [
                (i as f32 + 0.5 + (rng.f32() - 0.5) * jitter) * cell,
                (j as f32 + 0.5 + (rng.f32() - 0.5) * jitter) * cell,
            ];
            if inside(p) && spaced.try_add([p[0], 0.0, p[1]]) {
                out.push((p, rng));
            }
        }
    }
    (out, false)
}

fn catmull_rom(p: [[f32; 2]; 4], t: f32) -> [f32; 2] {
    let (t2, t3) = (t * t, t * t * t);
    [0, 1].map(|k| {
        0.5 * (2.0 * p[1][k]
            + (p[2][k] - p[0][k]) * t
            + (2.0 * p[0][k] - 5.0 * p[1][k] + 4.0 * p[2][k] - p[3][k]) * t2
            + (3.0 * p[1][k] - p[0][k] - 3.0 * p[2][k] + p[3][k]) * t3)
    })
}

/// The curve through `points` as a dense polyline (a closed curve ends where it began) with the
/// distance along it at every vertex.
fn spline_polyline(points: &[[f32; 2]], closed: bool) -> (Vec<[f32; 2]>, Vec<f32>) {
    const STEPS: usize = 16;
    let n = points.len();
    let at = |i: isize| {
        if closed {
            points[i.rem_euclid(n as isize) as usize]
        } else {
            points[i.clamp(0, n as isize - 1) as usize]
        }
    };
    let segments = if closed { n } else { n - 1 };
    let mut line = Vec::with_capacity(segments * STEPS + 1);
    for s in 0..segments as isize {
        for k in 0..STEPS {
            line.push(catmull_rom(
                [at(s - 1), at(s), at(s + 1), at(s + 2)],
                k as f32 / STEPS as f32,
            ));
        }
    }
    line.push(if closed { line[0] } else { points[n - 1] });
    let mut along = vec![0.0];
    for w in line.windows(2) {
        along.push(along[along.len() - 1] + (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]));
    }
    (line, along)
}

/// Position and unit direction at distance `s` along a polyline.
fn along_line(line: &[[f32; 2]], along: &[f32], s: f32) -> ([f32; 2], [f32; 2]) {
    let i = along.partition_point(|&a| a <= s).clamp(1, line.len() - 1);
    let (a, b) = (line[i - 1], line[i]);
    let seg = (along[i] - along[i - 1]).max(f32::MIN_POSITIVE);
    let t = ((s - along[i - 1]) / seg).clamp(0.0, 1.0);
    let len = (b[0] - a[0]).hypot(b[1] - a[1]).max(f32::MIN_POSITIVE);
    (
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t],
        [(b[0] - a[0]) / len, (b[1] - a[1]) / len],
    )
}

fn evaluate_spline(g: &SplineMass, ground: &dyn Fn(f32, f32) -> Option<f32>) -> Evaluation {
    let (line, along) = spline_polyline(&g.points, g.closed);
    let length = along[along.len() - 1];
    let (count, step) = match (g.spacing, g.closed) {
        (Spacing::Count(n), false) => (
            n as usize,
            if n > 1 { length / (n - 1) as f32 } else { 0.0 },
        ),
        (Spacing::Count(n), true) => (n as usize, length / n as f32),
        (Spacing::Every(d), false) => ((length / d).floor() as usize + 1, d),
        (Spacing::Every(d), true) => {
            let n = ((length / d).round() as usize).max(1);
            (n, length / n as f32)
        }
    };
    let truncated = count > MAX_PLACEMENTS;
    let mut placements = Vec::new();
    for k in 0..count.min(MAX_PLACEMENTS) {
        let mut rng = Rng::new(mix(g.seed, k as i64));
        let mut s = k as f32 * step + (rng.f32() - 0.5) * g.spacing_jitter * step;
        s = if g.closed {
            s.rem_euclid(length)
        } else {
            s.clamp(0.0, length)
        };
        let (p, d) = along_line(&line, &along, s);
        let (x, z) = (p[0] - d[1] * g.offset_m, p[1] + d[0] * g.offset_m);
        let y = ground(x, z).unwrap_or(0.0);
        placements.push(make(
            &g.asset_palette,
            (g.scale_min, g.scale_max),
            g.random_rotation,
            [x, y, z],
            &mut rng,
        ));
    }
    Evaluation {
        placements,
        truncated,
    }
}

fn distance_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (ab, ap) = ([b[0] - a[0], b[1] - a[1]], [p[0] - a[0], p[1] - a[1]]);
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 > 0.0 {
        ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - ab[0] * t).hypot(p[1] - a[1] - ab[1] * t)
}

fn stroke_hits(s: &Stroke, p: [f32; 2]) -> bool {
    if let [only] = s.points[..] {
        return distance_to_segment(p, only, only) <= s.radius_m;
    }
    s.points
        .windows(2)
        .any(|w| distance_to_segment(p, w[0], w[1]) <= s.radius_m)
}

/// Turns a generator into placements. `ground(x, z)` is the terrain height; a point with no
/// ground goes to y = 0. A [`SurfaceFill`] needs its owner's mesh and gives nothing here: use
/// [`evaluate_surface_fill`].
pub fn evaluate(generator: &Generator, ground: &dyn Fn(f32, f32) -> Option<f32>) -> Evaluation {
    let y_at = |p: [f32; 2]| [p[0], ground(p[0], p[1]).unwrap_or(0.0), p[1]];
    let done = |placements, truncated| Evaluation {
        placements,
        truncated,
    };
    if generator.validate().is_err() {
        return done(Vec::new(), false);
    }
    match generator {
        Generator::Single(g) => done(
            vec![Placement {
                asset: g.asset.clone(),
                position: g.position,
                rotation_y_deg: g.rotation_y_deg,
                scale: g.scale,
            }],
            false,
        ),
        Generator::SplineMass(g) => evaluate_spline(g, ground),
        Generator::AreaScatter(g) => {
            let b = bounds(g.outline.iter().copied());
            let (pts, truncated) = grid_points(
                b,
                g.density_per_m2,
                g.seed,
                g.position_jitter,
                g.min_distance_m,
                |p| point_in_polygon(p, &g.outline),
            );
            let scale = (g.scale_min, g.scale_max);
            let out = pts
                .into_iter()
                .map(|(p, mut rng)| {
                    make(
                        &g.asset_palette,
                        scale,
                        g.random_rotation,
                        y_at(p),
                        &mut rng,
                    )
                })
                .collect();
            done(out, truncated)
        }
        Generator::MixedBrush(g) => {
            let mut b = bounds(g.strokes.iter().filter(|s| !s.erase).flat_map(|s| {
                s.points.iter().flat_map(|p| {
                    [
                        [p[0] - s.radius_m, p[1] - s.radius_m],
                        [p[0] + s.radius_m, p[1] + s.radius_m],
                    ]
                })
            }));
            if b[0][0] > b[1][0] {
                b = [[0.0; 2]; 2]; // nothing painted: the walk below finds no coverage
            }
            let covered = |p| {
                g.strokes
                    .iter()
                    .rev()
                    .find(|s| stroke_hits(s, p))
                    .is_some_and(|s| !s.erase)
            };
            let (pts, truncated) = grid_points(b, g.density_per_m2, g.seed, 1.0, 0.0, covered);
            let scale = (g.scale_min, g.scale_max);
            let out = pts
                .into_iter()
                .map(|(p, mut rng)| make(&g.asset_palette, scale, true, y_at(p), &mut rng))
                .collect();
            done(out, truncated)
        }
        Generator::SurfaceFill(_) => done(Vec::new(), false),
        Generator::Cluster(g) => {
            let mut rng = Rng::new(g.seed);
            let count = (g.count as usize).min(MAX_PLACEMENTS);
            let out = (0..count)
                .map(|_| {
                    // Radius grows linearly with the draw, so the density falls off as 1/r.
                    let r = g.radius_m * rng.f32();
                    let a = std::f32::consts::TAU * rng.f32();
                    let p = [g.center[0] + r * a.cos(), g.center[1] + r * a.sin()];
                    make(
                        &g.asset_palette,
                        (g.scale_min, g.scale_max),
                        g.random_rotation,
                        y_at(p),
                        &mut rng,
                    )
                })
                .collect();
            done(out, g.count as usize > MAX_PLACEMENTS)
        }
    }
}

// ---- surfaces ----

/// Random points on the triangles of one face: each triangle gets `area * density` of them
/// (rounded up or down at random, by its own stream), uniform inside it, so the points always lie
/// on the face and a triangle's points do not change when others do. Triangles whose normal
/// points up less than `min_up` (the cosine of the steepest slope) are skipped. Each point comes
/// with its triangle's random stream, to go on drawing from.
fn triangle_points(
    model: &ImportedModel,
    face: usize,
    density: f32,
    seed: u64,
    min_up: f32,
) -> Vec<([f32; 3], Rng)> {
    let mut out = Vec::new();
    for (t, _) in model
        .triangle_faces
        .iter()
        .enumerate()
        .filter(|(_, f)| **f as usize == face)
    {
        let [a, b, c] = model.triangles[t].map(|i| model.positions[i as usize]);
        let (u, v) = (
            [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
            [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
        );
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let double_area = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if double_area == 0.0 || n[1] / double_area < min_up {
            continue;
        }
        let mut rng = Rng::new(mix(seed, t as i64));
        let expected = double_area / 2.0 * density;
        let count = expected.floor() as usize + usize::from(rng.f32() < expected.fract());
        for _ in 0..count.min(MAX_PLACEMENTS - out.len().min(MAX_PLACEMENTS)) {
            let (mut s, mut r) = (rng.f32(), rng.f32());
            if s + r > 1.0 {
                (s, r) = (1.0 - s, 1.0 - r);
            }
            out.push((std::array::from_fn(|k| a[k] + u[k] * s + v[k] * r), rng));
        }
    }
    out
}

/// A tuft of grass: where it stands and how tall it grows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuft {
    pub position: [f32; 3],
    pub rotation_y_deg: f32,
    pub height_m: f32,
}

/// Tufts for a lawn on face number `face` (an index into `model.faces`): about `density_per_m2`
/// per square metre of the face, never outside it, deterministic, with a height of 0.75-1.25
/// times the blade height. Capped at [`MAX_PLACEMENTS`].
pub fn lawn_scatter(model: &ImportedModel, face: usize, lawn: &Lawn3d) -> Vec<Tuft> {
    if !(lawn.density_per_m2.is_finite() && lawn.density_per_m2 > 0.0) {
        return Vec::new();
    }
    triangle_points(
        model,
        face,
        lawn.density_per_m2,
        lawn.seed,
        f32::NEG_INFINITY,
    )
    .into_iter()
    .map(|(position, mut rng)| Tuft {
        position,
        rotation_y_deg: 360.0 * rng.f32(),
        height_m: lawn.blade_height_m * (0.75 + 0.5 * rng.f32()),
    })
    .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceFillResult {
    pub placements: Vec<Placement>,
    /// Face names of the fill that the mesh no longer has; they are skipped, not forgotten.
    pub missing: Vec<String>,
    pub truncated: bool,
}

/// Evaluates a [`SurfaceFill`] against its owner's mesh: points by triangle area on the named
/// faces, on the surface, skipping triangles steeper than `max_slope_deg`.
pub fn evaluate_surface_fill(fill: &SurfaceFill, model: &ImportedModel) -> SurfaceFillResult {
    let mut result = SurfaceFillResult {
        placements: Vec::new(),
        missing: Vec::new(),
        truncated: false,
    };
    if Generator::SurfaceFill(fill.clone()).validate().is_err() {
        return result;
    }
    let min_up = fill.max_slope_deg.to_radians().cos();
    let mut spaced = Spaced::new(fill.min_distance_m);
    for name in &fill.faces {
        let Some(face) = model.faces.iter().position(|f| &f.name == name) else {
            result.missing.push(name.clone());
            continue;
        };
        for (p, mut rng) in triangle_points(
            model,
            face,
            fill.density_per_m2,
            mix(fill.seed, face as i64),
            min_up,
        ) {
            if result.placements.len() >= MAX_PLACEMENTS {
                result.truncated = true;
                return result;
            }
            if spaced.try_add(p) {
                let scale = (fill.scale_min, fill.scale_max);
                result.placements.push(make(
                    &fill.asset_palette,
                    scale,
                    fill.random_rotation,
                    p,
                    &mut rng,
                ));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::{Area, Extrusion, Shape, shape_mesh};

    fn palette() -> Vec<Weighted> {
        vec![
            Weighted {
                asset: "oak".into(),
                weight: 3.0,
            },
            Weighted {
                asset: "fern".into(),
                weight: 1.0,
            },
        ]
    }

    fn flat(_: f32, _: f32) -> Option<f32> {
        None
    }

    fn area(outline: Vec<[f32; 2]>, density: f32) -> AreaScatter {
        AreaScatter {
            id: "g1".into(),
            name: "Scatter".into(),
            layer: "planting".into(),
            asset_palette: palette(),
            outline,
            density_per_m2: density,
            position_jitter: 1.0,
            scale_min: 0.8,
            scale_max: 1.2,
            random_rotation: true,
            min_distance_m: 0.0,
            seed: 7,
        }
    }

    fn l_shape() -> Vec<[f32; 2]> {
        vec![
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 4.0],
            [4.0, 4.0],
            [4.0, 10.0],
            [0.0, 10.0],
        ]
    }

    fn inside_l(p: [f32; 3]) -> bool {
        point_in_polygon([p[0], p[2]], &l_shape())
    }

    #[test]
    fn area_scatter_is_deterministic_inside_and_weighted() {
        let g = Generator::AreaScatter(area(l_shape(), 4.0));
        let a = evaluate(&g, &flat);
        assert_eq!(a, evaluate(&g, &flat));
        assert!(!a.truncated);
        // 64 m2 at 4 per m2: about 256.
        assert!(
            (200..320).contains(&a.placements.len()),
            "{}",
            a.placements.len()
        );
        assert!(
            a.placements
                .iter()
                .all(|p| inside_l(p.position) && (0.8..=1.2).contains(&p.scale))
        );
        let mut big = area(
            vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]],
            2.0,
        );
        big.min_distance_m = 0.0;
        let all = evaluate(&Generator::AreaScatter(big), &flat).placements;
        let oaks = all.iter().filter(|p| p.asset == "oak").count() as f32 / all.len() as f32;
        assert!((oaks - 0.75).abs() < 0.02, "{oaks}");
    }

    #[test]
    fn area_scatter_points_stay_when_far_edge_moves() {
        let before = evaluate(&Generator::AreaScatter(area(l_shape(), 4.0)), &flat).placements;
        let mut longer = l_shape();
        longer[1] = [14.0, 0.0];
        longer[2] = [14.0, 4.0];
        let after = evaluate(&Generator::AreaScatter(area(longer, 4.0)), &flat).placements;
        let near = |v: &[Placement]| {
            v.iter()
                .filter(|p| p.position[0] < 9.0)
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(near(&before), near(&after));
    }

    #[test]
    fn min_distance_and_ground_are_honoured() {
        let mut g = area(l_shape(), 6.0);
        g.min_distance_m = 0.5;
        let hill = |x: f32, _: f32| Some(x * 0.1);
        let v = evaluate(&Generator::AreaScatter(g), &hill).placements;
        assert!(!v.is_empty());
        for (i, a) in v.iter().enumerate() {
            assert!((a.position[1] - a.position[0] * 0.1).abs() < 1e-5);
            for b in &v[i + 1..] {
                let d = (a.position[0] - b.position[0]).hypot(a.position[2] - b.position[2]);
                assert!(d >= 0.5 - 1e-4, "{d}");
            }
        }
    }

    #[test]
    fn typo_density_is_capped_and_flagged() {
        let g = Generator::AreaScatter(area(l_shape(), 1.0e6));
        let e = evaluate(&g, &flat);
        assert!(e.truncated && e.placements.len() == MAX_PLACEMENTS);
        let line = Generator::SplineMass(SplineMass {
            id: "g".into(),
            name: "Hedge".into(),
            layer: "planting".into(),
            asset_palette: palette(),
            points: vec![[0.0, 0.0], [100.0, 0.0]],
            closed: false,
            spacing: Spacing::Every(1.0e-5),
            spacing_jitter: 0.0,
            offset_m: 0.0,
            scale_min: 1.0,
            scale_max: 1.0,
            random_rotation: false,
            seed: 1,
        });
        let e = evaluate(&line, &flat);
        assert!(e.truncated && e.placements.len() == MAX_PLACEMENTS);
    }

    fn spline(closed: bool, spacing: Spacing) -> SplineMass {
        SplineMass {
            id: "g2".into(),
            name: "Hedge".into(),
            layer: "planting".into(),
            asset_palette: palette(),
            points: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            closed,
            spacing,
            spacing_jitter: 0.3,
            offset_m: 0.0,
            scale_min: 1.0,
            scale_max: 1.5,
            random_rotation: false,
            seed: 3,
        }
    }

    #[test]
    fn spline_passes_through_its_points_and_is_deterministic() {
        let mut g = spline(false, Spacing::Count(11));
        g.spacing_jitter = 0.0;
        let e = evaluate(&Generator::SplineMass(g.clone()), &flat).placements;
        assert_eq!(e.len(), 11);
        assert_eq!(e[0].position, [0.0, 0.0, 0.0]);
        let last = e[10].position;
        assert!(
            last[0].abs() < 1e-3 && (last[2] - 10.0).abs() < 1e-3,
            "{last:?}"
        );
        let j = Generator::SplineMass(spline(true, Spacing::Every(1.0)));
        assert_eq!(evaluate(&j, &flat), evaluate(&j, &flat));
        let n = evaluate(&j, &flat).placements.len();
        assert!((38..=46).contains(&n), "{n}");
        // Offset moves items to the right of travel: heading east from the first point, that is +z.
        g.offset_m = 1.0;
        let shifted = evaluate(&Generator::SplineMass(g), &flat).placements;
        assert!((shifted[0].position[2] - 1.0).abs() < 0.02);
    }

    #[test]
    fn brush_follows_strokes_and_erases() {
        let stroke = |points: Vec<[f32; 2]>, radius_m, erase| Stroke {
            points,
            radius_m,
            erase,
        };
        let mut g = MixedBrush {
            id: "g3".into(),
            name: "Brush".into(),
            layer: "planting".into(),
            asset_palette: palette(),
            strokes: vec![stroke(vec![[0.0, 0.0], [10.0, 0.0]], 2.0, false)],
            density_per_m2: 5.0,
            scale_min: 1.0,
            scale_max: 1.0,
            seed: 9,
        };
        let all = evaluate(&Generator::MixedBrush(g.clone()), &flat).placements;
        assert!(all.len() > 100);
        assert!(all.iter().all(|p| distance_to_segment(
            [p.position[0], p.position[2]],
            [0.0, 0.0],
            [10.0, 0.0]
        ) <= 2.0));
        g.strokes.push(stroke(vec![[5.0, 0.0]], 3.0, true));
        let cut = evaluate(&Generator::MixedBrush(g.clone()), &flat).placements;
        assert!(cut.len() < all.len());
        assert!(
            cut.iter()
                .all(|p| (p.position[0] - 5.0).hypot(p.position[2]) > 3.0)
        );
        g.strokes.push(stroke(vec![[5.0, 0.0]], 1.0, false));
        let back = evaluate(&Generator::MixedBrush(g.clone()), &flat).placements;
        assert!(
            back.iter()
                .any(|p| (p.position[0] - 5.0).hypot(p.position[2]) <= 1.0)
        );
        assert_eq!(
            back,
            evaluate(&Generator::MixedBrush(g.clone()), &flat).placements
        );
        g.strokes.clear();
        assert!(
            evaluate(&Generator::MixedBrush(g), &flat)
                .placements
                .is_empty()
        );
    }

    #[test]
    fn cluster_is_within_radius_and_centre_weighted() {
        let g = Generator::Cluster(Cluster {
            id: "g4".into(),
            name: "Clump".into(),
            layer: "planting".into(),
            asset_palette: palette(),
            center: [5.0, -3.0],
            radius_m: 2.0,
            count: 4000,
            scale_min: 1.0,
            scale_max: 1.0,
            random_rotation: true,
            seed: 5,
        });
        let e = evaluate(&g, &flat);
        assert_eq!(e, evaluate(&g, &flat));
        assert_eq!(e.placements.len(), 4000);
        let d = |p: &Placement| (p.position[0] - 5.0).hypot(p.position[2] + 3.0);
        assert!(e.placements.iter().all(|p| d(p) <= 2.0 + 1e-4));
        let inner = e.placements.iter().filter(|p| d(p) < 1.0).count();
        // Half the radius is a quarter of the area, but holds about half the items.
        assert!(inner > 1800, "{inner}");
    }

    fn lawn_area() -> (Shape, ImportedModel) {
        let s = Shape::Area(Area {
            id: "S1".into(),
            name: "Lawn".into(),
            layer: "shapes".into(),
            outline: vec![[0.0, 0.0], [6.0, 0.0], [6.0, 3.0], [0.0, 3.0]],
            elevation: 0.25,
        });
        let m = shape_mesh(&s).unwrap();
        (s, m)
    }

    fn fill(faces: Vec<&str>) -> SurfaceFill {
        SurfaceFill {
            id: "g5".into(),
            name: "Fill".into(),
            layer: "planting".into(),
            asset_palette: palette(),
            owner: "S1".into(),
            faces: faces.into_iter().map(String::from).collect(),
            density_per_m2: 10.0,
            position_jitter: 1.0,
            scale_min: 1.0,
            scale_max: 1.0,
            random_rotation: false,
            min_distance_m: 0.0,
            max_slope_deg: 45.0,
            seed: 11,
        }
    }

    #[test]
    fn surface_fill_lies_on_named_faces_and_reports_missing() {
        let (_, m) = lawn_area();
        let f = fill(vec!["S1:top", "S1:gone"]);
        let r = evaluate_surface_fill(&f, &m);
        assert_eq!(r, evaluate_surface_fill(&f, &m));
        assert_eq!(r.missing, ["S1:gone"]);
        assert!(
            (120..240).contains(&r.placements.len()),
            "{}",
            r.placements.len()
        );
        for p in &r.placements {
            let [x, y, z] = p.position;
            assert!(
                (y - 0.25).abs() < 1e-5 && (0.0..=6.0).contains(&x) && (0.0..=3.0).contains(&z)
            );
        }
    }

    #[test]
    fn surface_fill_skips_steep_triangles_and_lawn_tufts_stay_on_the_face() {
        let block = Shape::Extrusion(Extrusion {
            id: "S2".into(),
            name: "Block".into(),
            layer: "shapes".into(),
            outline: vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]],
            closed: true,
            base: 0.0,
            height: 2.0,
            thickness: 0.2,
        });
        let m = shape_mesh(&block).unwrap();
        let names: Vec<&str> = m.faces.iter().map(|f| f.name.as_str()).collect();
        let r = evaluate_surface_fill(&fill(names), &m);
        // Only the roof (the walls are vertical, the bottom faces down).
        assert!(!r.placements.is_empty() && r.placements.iter().all(|p| p.position[1] == 2.0));
        let lawn = Lawn3d {
            density_per_m2: 50.0,
            blade_height_m: 0.05,
            seed: 2,
        };
        let top = m.faces.iter().position(|f| f.name == "S2:top").unwrap();
        let tufts = lawn_scatter(&m, top, &lawn);
        assert_eq!(tufts, lawn_scatter(&m, top, &lawn));
        assert!((600..1000).contains(&tufts.len()), "{}", tufts.len());
        assert!(tufts.iter().all(|t| t.position[1] == 2.0
            && (0.0..=4.0).contains(&t.position[0])
            && (0.0..=4.0).contains(&t.position[2])
            && (0.0375..=0.0625).contains(&t.height_m)));
        let wall = m.faces.iter().position(|f| f.name == "S2:side(0)").unwrap();
        // A wall is a face like any other for lawn_scatter; tufts lie in its plane, z = 0.
        assert!(
            lawn_scatter(&m, wall, &lawn)
                .iter()
                .all(|t| t.position[2].abs() < 1e-5)
        );
    }

    #[test]
    fn evaluating_ten_thousand_points_is_fast() {
        let g = Generator::AreaScatter(area(
            vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]],
            1.0,
        ));
        let t = std::time::Instant::now();
        let n = evaluate(&g, &flat).placements.len();
        eprintln!("evaluate: {n} placements in {:?}", t.elapsed());
        assert!(n > 9000);
    }
}
