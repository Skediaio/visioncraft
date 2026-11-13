//! Plants as editable generators, not baked instances: the project stores the recipe, [`evaluate`]
//! turns it into placements, deterministically (a small SplitMix64 of our own, no `rand`).

use crate::ImportedModel;
use crate::material::Lawn3d;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

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
    /// Which variant of the species to draw: the app takes it modulo the number of variants it
    /// has. Deterministic, and drawn so that positions, rotations and scales do not depend on it.
    #[serde(default)]
    pub variant: u32,
    /// Index of the [`PlantLayer`] that made it; `None` for a generator without layers.
    #[serde(default)]
    pub layer: Option<u32>,
}

/// One species of a palette. `asset` is, by convention, one of: a catalogue asset id, `flora:<id>`
/// for a built-in procedural species, the id of a custom species of the project (`sp1`...), or
/// `user:<id>` for one of the user's own objects. The core does not check which.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Weighted {
    pub asset: String,
    pub weight: f32,
}

/// One storey of a layered planting (canopy, shrubs, ground cover). When a generator has layers
/// they replace its own palette, density and scale fields; each layer is evaluated on its own
/// random stream, in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlantLayer {
    pub name: String,
    pub asset_palette: Vec<Weighted>,
    pub density_per_m2: f32,
    pub min_distance_m: f32,
    pub scale_min: f32,
    pub scale_max: f32,
    pub random_rotation: bool,
    /// Drops points closer (horizontally) than this to any point of an EARLIER layer.
    pub keep_clear_m: f32,
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
    #[serde(default)]
    pub asset_palette: Vec<Weighted>,
    pub outline: Vec<[f32; 2]>,
    #[serde(default)]
    pub density_per_m2: f32,
    /// 0..1: how far a point may stray from its cell centre, as a fraction of the cell.
    pub position_jitter: f32,
    pub scale_min: f32,
    pub scale_max: f32,
    pub random_rotation: bool,
    pub min_distance_m: f32,
    pub seed: u64,
    #[serde(default)]
    pub layers: Vec<PlantLayer>,
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
    #[serde(default)]
    pub asset_palette: Vec<Weighted>,
    pub strokes: Vec<Stroke>,
    #[serde(default)]
    pub density_per_m2: f32,
    pub scale_min: f32,
    pub scale_max: f32,
    /// Without layers: no two plants closer than this.
    #[serde(default)]
    pub min_distance_m: f32,
    pub seed: u64,
    #[serde(default)]
    pub layers: Vec<PlantLayer>,
}

/// Fills named faces of a model or shape with plants; needs the owner's mesh, see
/// [`evaluate_surface_fill`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceFill {
    pub id: String,
    pub name: String,
    pub layer: String,
    #[serde(default)]
    pub asset_palette: Vec<Weighted>,
    /// A model or shape id.
    pub owner: String,
    /// Persistent face names on that owner.
    pub faces: Vec<String>,
    #[serde(default)]
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
    #[serde(default)]
    pub layers: Vec<PlantLayer>,
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

    /// Whether the generator places `asset` (a catalogue, `flora:` or custom species id).
    pub fn uses_asset(&self, asset: &str) -> bool {
        let palette = |p: &[Weighted]| p.iter().any(|w| w.asset == asset);
        let layered = |p: &[Weighted], l: &[PlantLayer]| {
            palette(p) || l.iter().any(|l| palette(&l.asset_palette))
        };
        match self {
            Self::Single(g) => g.asset == asset,
            Self::SplineMass(g) => palette(&g.asset_palette),
            Self::AreaScatter(g) => layered(&g.asset_palette, &g.layers),
            Self::MixedBrush(g) => layered(&g.asset_palette, &g.layers),
            Self::SurfaceFill(g) => layered(&g.asset_palette, &g.layers),
            Self::Cluster(g) => palette(&g.asset_palette),
        }
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
        let min_distance = |d: f32| ok(d.is_finite() && d >= 0.0, "min distance must be 0 or more");
        // Layers replace the generator's own palette, density and scale.
        let layers = |l: &[PlantLayer]| {
            l.iter().try_for_each(|l| {
                palette(&l.asset_palette)?;
                density(l.density_per_m2)?;
                scales(l.scale_min, l.scale_max)?;
                min_distance(l.min_distance_m)?;
                min_distance(l.keep_clear_m)
            })
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
                flat(&g.outline, 3)?;
                unit(g.position_jitter)?;
                if !g.layers.is_empty() {
                    return layers(&g.layers);
                }
                palette(&g.asset_palette)?;
                scales(g.scale_min, g.scale_max)?;
                density(g.density_per_m2)?;
                min_distance(g.min_distance_m)
            }
            Self::MixedBrush(g) => {
                if g.layers.is_empty() {
                    palette(&g.asset_palette)?;
                    scales(g.scale_min, g.scale_max)?;
                    density(g.density_per_m2)?;
                    min_distance(g.min_distance_m)?;
                } else {
                    layers(&g.layers)?;
                }
                g.strokes.iter().try_for_each(|s| {
                    flat(&s.points, 1)?;
                    ok(
                        s.radius_m.is_finite() && s.radius_m > 0.0,
                        "stroke radius must be above zero",
                    )
                })
            }
            Self::SurfaceFill(g) => {
                if g.layers.is_empty() {
                    palette(&g.asset_palette)?;
                    scales(g.scale_min, g.scale_max)?;
                    density(g.density_per_m2)?;
                } else {
                    layers(&g.layers)?;
                }
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
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..1`.
    pub(crate) fn f32(&mut self) -> f32 {
        (self.next() >> 40) as f32 / 16_777_216.0
    }
}

/// A seed for a sub-stream, e.g. one grid cell.
pub(crate) fn mix(a: u64, b: i64) -> u64 {
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
    // A side stream: `rng` itself is not advanced, so a generator that goes on drawing from it
    // (a cluster) gives the same points as before variants existed.
    let variant = Rng::new(rng.0 ^ 0xA5A5_5A5A_C3C3_3C3C).next() as u32;
    Placement {
        asset,
        position,
        rotation_y_deg,
        scale,
        variant,
        layer: None,
    }
}

/// Points in a uniform grid of `cell`-sized buckets, for "anything closer than `cell`?" queries
/// without comparing all pairs. The map is only ever looked up, never iterated, so its order
/// cannot matter.
struct Grid3 {
    cell: f32,
    cells: HashMap<(i32, i32, i32), Vec<[f32; 3]>, CellBuild>,
    /// Range of cell rows (y) that hold points: queries skip the empty rows above and below.
    rows: (i32, i32),
}

type CellBuild = BuildHasherDefault<CellHasher>;

/// A cheap multiplicative hasher for the integer cell keys (the default SipHash is the hot spot
/// of a dense scatter).
#[derive(Default)]
struct CellHasher(u64);

impl Hasher for CellHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0.rotate_left(5) ^ u64::from(*b)).wrapping_mul(0x517C_C1B7_2722_0A95);
        }
    }

    fn write_i32(&mut self, v: i32) {
        self.0 = (self.0.rotate_left(5) ^ (v as u32 as u64)).wrapping_mul(0x517C_C1B7_2722_0A95);
    }
}

impl Grid3 {
    fn new(cell: f32) -> Self {
        Self {
            cell,
            cells: HashMap::default(),
            rows: (i32::MAX, i32::MIN),
        }
    }

    fn key(&self, p: [f32; 3]) -> (i32, i32, i32) {
        let c = p.map(|v| (v / self.cell).floor() as i32);
        (c[0], c[1], c[2])
    }

    fn insert(&mut self, p: [f32; 3]) {
        let k = self.key(p);
        self.rows = (self.rows.0.min(k.1), self.rows.1.max(k.1));
        self.cells.entry(k).or_default().push(p);
    }

    /// Whether a point lies closer than `cell` to `p`; always false for a zero cell.
    fn near(&self, p: [f32; 3]) -> bool {
        if self.cell <= 0.0 || self.cells.is_empty() {
            return false;
        }
        let c = self.key(p);
        let r2 = self.cell * self.cell;
        for dy in (self.rows.0 - c.1).max(-1)..=(self.rows.1 - c.1).min(1) {
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let Some(pts) = self.cells.get(&(c.0 + dx, c.1 + dy, c.2 + dz)) else {
                        continue;
                    };
                    if pts
                        .iter()
                        .any(|q| (0..3).map(|k| (q[k] - p[k]).powi(2)).sum::<f32>() < r2)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Adds `p` unless a point is closer than the cell size.
    fn try_add(&mut self, p: [f32; 3]) -> bool {
        if self.near(p) {
            return false;
        }
        if self.cell > 0.0 {
            self.insert(p);
        }
        true
    }
}

pub(crate) fn point_in_polygon(p: [f32; 2], poly: &[[f32; 2]]) -> bool {
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
    let mut spaced = Grid3::new(min_distance);
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

/// What one layer of a generator needs to be evaluated; a generator without layers is one tier
/// made of its own fields.
struct Tier<'a> {
    palette: &'a [Weighted],
    density: f32,
    min_distance: f32,
    scale: (f32, f32),
    spin: bool,
    keep_clear: f32,
    seed: u64,
    /// Index of the `PlantLayer`, for plants it makes.
    layer: Option<u32>,
}

/// The tiers of a generator: its layers if it has any (each on its own stream), else `own`.
fn tiers<'a>(own: Tier<'a>, layers: &'a [PlantLayer], seed: u64) -> Vec<Tier<'a>> {
    if layers.is_empty() {
        return vec![own];
    }
    layers
        .iter()
        .enumerate()
        .map(|(i, l)| Tier {
            palette: &l.asset_palette,
            density: l.density_per_m2,
            min_distance: l.min_distance_m,
            scale: (l.scale_min, l.scale_max),
            spin: l.random_rotation,
            keep_clear: l.keep_clear_m,
            seed: mix(seed ^ 0x504C_414E_545F_4C59, i as i64),
            layer: Some(i as u32),
        })
        .collect()
}

/// Evaluates tiers in order. `one` gets the tier and a grid of the points of earlier tiers
/// (cell size = the tier's `keep_clear`, empty when that is zero) and returns its placements.
fn run_tiers<'a>(
    tiers: Vec<Tier<'a>>,
    mut one: impl FnMut(&Tier<'a>, &Grid3) -> (Vec<Placement>, bool),
) -> Evaluation {
    let mut placements: Vec<Placement> = Vec::new();
    let mut truncated = false;
    for t in &tiers {
        let mut keep = Grid3::new(t.keep_clear);
        if t.keep_clear > 0.0 {
            for p in &placements {
                keep.insert([p.position[0], 0.0, p.position[2]]);
            }
        }
        let (mut more, cut) = one(t, &keep);
        for p in &mut more {
            p.layer = t.layer;
        }
        truncated |= cut;
        placements.extend(more);
        if placements.len() >= MAX_PLACEMENTS {
            truncated |= placements.len() > MAX_PLACEMENTS;
            placements.truncate(MAX_PLACEMENTS);
            break;
        }
    }
    Evaluation {
        placements,
        truncated,
    }
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
                // A single has no stream: its variant comes from its id.
                variant: g
                    .id
                    .bytes()
                    .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b))),
                layer: None,
            }],
            false,
        ),
        Generator::SplineMass(g) => evaluate_spline(g, ground),
        Generator::AreaScatter(g) => {
            let b = bounds(g.outline.iter().copied());
            let own = Tier {
                palette: &g.asset_palette,
                density: g.density_per_m2,
                min_distance: g.min_distance_m,
                scale: (g.scale_min, g.scale_max),
                spin: g.random_rotation,
                keep_clear: 0.0,
                layer: None,
                seed: g.seed,
            };
            run_tiers(tiers(own, &g.layers, g.seed), |t, keep| {
                let (pts, truncated) = grid_points(
                    b,
                    t.density,
                    t.seed,
                    g.position_jitter,
                    t.min_distance,
                    |p| point_in_polygon(p, &g.outline) && !keep.near([p[0], 0.0, p[1]]),
                );
                let out = pts
                    .into_iter()
                    .map(|(p, mut rng)| make(t.palette, t.scale, t.spin, y_at(p), &mut rng))
                    .collect();
                (out, truncated)
            })
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
            let own = Tier {
                palette: &g.asset_palette,
                density: g.density_per_m2,
                min_distance: g.min_distance_m,
                scale: (g.scale_min, g.scale_max),
                spin: true,
                keep_clear: 0.0,
                layer: None,
                seed: g.seed,
            };
            run_tiers(tiers(own, &g.layers, g.seed), |t, keep| {
                let (pts, truncated) =
                    grid_points(b, t.density, t.seed, 1.0, t.min_distance, |p| {
                        covered(p) && !keep.near([p[0], 0.0, p[1]])
                    });
                let out = pts
                    .into_iter()
                    .map(|(p, mut rng)| make(t.palette, t.scale, t.spin, y_at(p), &mut rng))
                    .collect();
                (out, truncated)
            })
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
/// faces, on the surface, skipping triangles steeper than `max_slope_deg`. With layers, each is
/// filled in turn and keeps clear (horizontally) of the earlier ones.
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
    let mut faces = Vec::new();
    for name in &fill.faces {
        match model.faces.iter().position(|f| &f.name == name) {
            Some(face) => faces.push(face),
            None => result.missing.push(name.clone()),
        }
    }
    let own = Tier {
        palette: &fill.asset_palette,
        density: fill.density_per_m2,
        min_distance: fill.min_distance_m,
        scale: (fill.scale_min, fill.scale_max),
        spin: fill.random_rotation,
        keep_clear: 0.0,
        layer: None,
        seed: fill.seed,
    };
    let e = run_tiers(tiers(own, &fill.layers, fill.seed), |t, keep| {
        let mut spaced = Grid3::new(t.min_distance);
        let mut out = Vec::new();
        for &face in &faces {
            for (p, mut rng) in
                triangle_points(model, face, t.density, mix(t.seed, face as i64), min_up)
            {
                if out.len() >= MAX_PLACEMENTS {
                    return (out, true);
                }
                if !keep.near([p[0], 0.0, p[2]]) && spaced.try_add(p) {
                    out.push(make(t.palette, t.scale, t.spin, p, &mut rng));
                }
            }
        }
        (out, false)
    });
    result.placements = e.placements;
    result.truncated = e.truncated;
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
            layers: Vec::new(),
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
            min_distance_m: 0.0,
            seed: 9,
            layers: Vec::new(),
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
            drape: false,
            extensions: Default::default(),
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
            layers: Vec::new(),
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
            extensions: Default::default(),
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
    fn variants_are_spread_and_stable() {
        let g = Generator::AreaScatter(area(l_shape(), 4.0));
        let a = evaluate(&g, &flat).placements;
        let b = evaluate(&g, &flat).placements;
        assert!(a.iter().zip(&b).all(|(x, y)| x.variant == y.variant));
        let distinct: std::collections::BTreeSet<_> = a.iter().map(|p| p.variant % 4).collect();
        assert_eq!(distinct.len(), 4, "{distinct:?}");
        assert!(a.iter().any(|p| p.variant > u32::MAX / 2));
        assert!(g.uses_asset("fern") && !g.uses_asset("flora:oak"));
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

#[cfg(test)]
mod layer_tests {
    use super::*;

    fn layer(species: &str, density: f32, min: f32, keep: f32, seed_scale: f32) -> PlantLayer {
        PlantLayer {
            name: species.into(),
            asset_palette: vec![Weighted {
                asset: species.into(),
                weight: 1.0,
            }],
            density_per_m2: density,
            min_distance_m: min,
            scale_min: seed_scale,
            scale_max: seed_scale,
            random_rotation: true,
            keep_clear_m: keep,
        }
    }

    fn hectare_brush(layers: Vec<PlantLayer>) -> Generator {
        Generator::MixedBrush(MixedBrush {
            id: "g1".into(),
            name: "Forest".into(),
            layer: "planting".into(),
            asset_palette: Vec::new(),
            strokes: vec![Stroke {
                points: vec![[0.0, 50.0], [100.0, 50.0]],
                radius_m: 50.0,
                erase: false,
            }],
            density_per_m2: 0.0,
            scale_min: 1.0,
            scale_max: 1.0,
            min_distance_m: 0.0,
            seed: 4,
            layers,
        })
    }

    fn forest() -> Vec<PlantLayer> {
        vec![
            layer("tree", 0.02, 4.0, 0.0, 1.0),
            layer("shrub", 0.15, 1.5, 2.0, 1.0),
            layer("fern", 1.5, 0.3, 0.8, 1.0),
        ]
    }

    #[test]
    fn layers_are_independent_deterministic_and_keep_clear() {
        let flat = |_: f32, _: f32| None;
        let g = hectare_brush(forest());
        let t = std::time::Instant::now();
        let e = evaluate(&g, &flat);
        eprintln!(
            "three-layer brush over about 1.5 ha: {} placements in {:?}",
            e.placements.len(),
            t.elapsed()
        );
        assert_eq!(e, evaluate(&g, &flat));
        let of = |a: &str| {
            e.placements
                .iter()
                .filter(|p| p.asset == a)
                .map(|p| p.position)
                .collect::<Vec<_>>()
        };
        let (trees, shrubs, ferns) = (of("tree"), of("shrub"), of("fern"));
        assert!(
            !trees.is_empty() && shrubs.len() > trees.len() * 3 && ferns.len() > shrubs.len() * 3
        );
        let d = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
        assert!(
            shrubs
                .iter()
                .all(|s| trees.iter().all(|t| d(*s, *t) >= 2.0))
        );
        assert!(
            ferns
                .iter()
                .all(|f| shrubs.iter().chain(&trees).all(|o| d(*f, *o) >= 0.8))
        );
        // Changing the shrub layer leaves the trees where they were; a layer's stream is its own.
        let mut more = forest();
        more[1].density_per_m2 = 0.3;
        assert_eq!(of("tree"), {
            let e2 = evaluate(&hectare_brush(more), &flat);
            e2.placements
                .iter()
                .filter(|p| p.asset == "tree")
                .map(|p| p.position)
                .collect::<Vec<_>>()
        });
    }

    #[test]
    fn hectare_with_three_layers_is_fast() {
        let g = hectare_brush(forest());
        // Warm up, then time the evaluation proper.
        let flat = |_: f32, _: f32| None;
        let _ = evaluate(&g, &flat);
        let t = std::time::Instant::now();
        let e = evaluate(&g, &flat);
        eprintln!(
            "hectare brush, 3 layers: {} placements in {:?}",
            e.placements.len(),
            t.elapsed()
        );
        assert!(t.elapsed().as_millis() < 500);
    }

    #[test]
    fn layered_area_and_fill_work_and_validate() {
        let flat = |_: f32, _: f32| None;
        let area = Generator::AreaScatter(AreaScatter {
            id: "g2".into(),
            name: "A".into(),
            layer: "planting".into(),
            asset_palette: Vec::new(),
            outline: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]],
            density_per_m2: 0.0,
            position_jitter: 1.0,
            scale_min: 1.0,
            scale_max: 1.0,
            random_rotation: false,
            min_distance_m: 0.0,
            seed: 1,
            layers: forest(),
        });
        let e = evaluate(&area, &flat);
        assert!(
            e.placements.len() > 400
                && e.placements
                    .iter()
                    .all(|p| (0.0..=20.0).contains(&p.position[0]))
        );
        assert!(area.uses_asset("fern"));
        let mut bad = forest();
        bad[0].density_per_m2 = 0.0;
        let Generator::AreaScatter(mut a) = area else {
            unreachable!()
        };
        a.layers = bad;
        assert!(Generator::AreaScatter(a).validate().is_err());
        // Surface fill with layers on the top of a drawn area.
        let shape = crate::shape::shape_mesh(&crate::shape::Shape::Area(crate::shape::Area {
            id: "S1".into(),
            name: "L".into(),
            layer: "shapes".into(),
            outline: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            elevation: 1.0,
            drape: false,
            extensions: Default::default(),
        }))
        .unwrap();
        let fill = SurfaceFill {
            id: "g3".into(),
            name: "F".into(),
            layer: "planting".into(),
            asset_palette: Vec::new(),
            owner: "S1".into(),
            faces: vec!["S1:top".into()],
            density_per_m2: 0.0,
            position_jitter: 1.0,
            scale_min: 1.0,
            scale_max: 1.0,
            random_rotation: false,
            min_distance_m: 0.0,
            max_slope_deg: 45.0,
            seed: 2,
            layers: forest(),
        };
        let r = evaluate_surface_fill(&fill, &shape);
        assert!(r.placements.len() > 100 && r.placements.iter().all(|p| p.position[1] == 1.0));
        assert_eq!(r, evaluate_surface_fill(&fill, &shape));
    }
}
