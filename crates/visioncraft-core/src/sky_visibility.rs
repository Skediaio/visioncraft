//! How much of the sky each point of a garden sees, baked once per change of the design and
//! relit every frame: shaded and backlit places get the sky's light in proportion to the sky
//! they see, so the ground under a crown and the foot of a wall go gently darker and the inside
//! of a backlit crown keeps the sky that comes through its leaves.
//!
//! The method is the sky visibility of Ghost of Tsushima (Jasmin Patry, "Real-Time Samurai
//! Cinema: Lighting, Atmosphere, and Tonemapping in Ghost of Tsushima", SIGGRAPH 2021 Advances
//! in Real-Time Rendering): a grid of probes, each holding the visibility of the sky over the
//! whole sphere as order-2 (nine coefficient) spherical harmonics, baked offline; at run time the
//! sky's radiance, projected to the same harmonics, is multiplied by the visibility and
//! convolved with the clamped cosine, so the light follows the time of day and the weather.
//! The product of two functions given as harmonics is projected back onto the nine with the
//! triple-product integrals of the real basis (Sloan, "Stupid Spherical Harmonics (SH) Tricks",
//! GDC 2008, section on products), the cosine convolution is Ramamoorthi and Hanrahan's ("An
//! Efficient Representation for Irradiance Environment Maps", SIGGRAPH 2001).
//!
//! The grid follows the ground: horizontally [`CELL_M`] apart (one metre: about a fifth of a
//! garden tree's crown, and the scale over which a wall's shade on the ground changes; finer
//! than that is the screen-space occlusion's), vertically in [`LAYERS`] layers whose spacing
//! grows with the square of the height over the ground, so the layers crowd near the ground
//! where the shade of walls and crowns changes fastest (12 cm apart at the bottom of a 15 m
//! grid, 2.6 m at the top). Each probe casts [`RAYS`] rays (a Fibonacci sphere, turned by its
//! own angle so neighbours do not share their gaps) against the triangles of buildings, shapes
//! and models, the terrain, and the crowns of trees and shrubs, which are ellipsoids of leaves
//! a ray crosses with the Beer-Lambert transmittance of its chord, the extinction set by the
//! species' leaf area (`crown_extinction`); a probe among a crown's leaves leaves that crown out
//! (`Tracer::near`), whose own shade the renderer keeps per leaf. A ray that comes down onto the ground or onto the
//! top of something (a roof seen from above, a deck) counts as open: the sky's harmonics already
//! carry the ground's light below the horizon.
//!
//! Also baked per probe: whether it is outdoors (`valid`: probes inside a building or a wall see
//! no sky at all and are left out of the interpolation, as the interior probes of the talk), and
//! the mean reflectance of what it sees instead of sky (`albedo`), for the light those occluders
//! bounce on (the talk bakes a bounced sky visibility; this keeps a single number, lit at run
//! time by half the sky and part of the sun).

use std::f32::consts::PI;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

/// Probe spacing over the ground, metres; grows for large sites to stay within `MAX_PROBES`.
pub const CELL_M: f32 = 1.0;
/// Layers of probes over the ground.
pub const LAYERS: u32 = 12;
/// The lowest layer's height over the ground (m): over the paving and lawn it lies on.
pub const LIFT_M: f32 = 0.1;
/// Rays per probe.
pub const RAYS: usize = 128;
const MAX_PROBES: usize = 120_000;
/// Room round the occluders, metres: the renderer fades the grid out over its outer two cells.
const PAD_M: f32 = 4.0;
/// The reflectance of a wall, roof or hill a ray meets (plaster, brick, stone, soil).
// shortcut: one grey for every occluder; the materials are not read.
pub const SOLID_ALBEDO: f32 = 0.3;
/// The share of the light a leaf crown scatters on (reflected and transmitted, R + T of a green
/// leaf over the visible: Jacquemoud & Baret, "PROSPECT", Remote Sens. Environ. 34, 1990).
pub const LEAF_ALBEDO: f32 = 0.2;
/// A probe is indoors when fewer than this share of its upward rays leave without meeting a
/// building, a wall or the terrain.
const INDOOR_SHARE: f32 = 0.05;

// ---- spherical harmonics ---------------------------------------------------------------------

/// The real order-2 spherical harmonics at unit `d`, in the order of the renderer's sky
/// harmonics (env.wgsl `sh_basis`).
pub fn sh_basis(d: [f32; 3]) -> [f32; 9] {
    let [x, y, z] = d;
    [
        0.282_095,
        0.488_603 * y,
        0.488_603 * z,
        0.488_603 * x,
        1.092_548 * x * y,
        1.092_548 * y * z,
        0.315_392 * (3.0 * z * z - 1.0),
        1.092_548 * x * z,
        0.546_274 * (x * x - y * y),
    ]
}

/// The band of each coefficient.
const BAND: [usize; 9] = [0, 1, 1, 1, 2, 2, 2, 2, 2];
/// The clamped cosine's band factors (Ramamoorthi and Hanrahan 2001): `A_l`.
pub const COSINE_BAND: [f32; 3] = [PI, 2.0 * PI / 3.0, PI / 4.0];

/// The triple-product integrals `T_ijk = ∫ Y_i Y_j Y_k` of the basis that are not zero, as
/// `(i, j, k, T)` over all orderings. Found once by a quadrature exact for these polynomials
/// (Gauss-Legendre in the cosine, eight nodes; sixteen even steps round the axis).
pub fn sh_triple() -> &'static [(usize, usize, usize, f32)] {
    static T: OnceLock<Vec<(usize, usize, usize, f32)>> = OnceLock::new();
    T.get_or_init(|| {
        let nodes = gauss_legendre(8);
        let steps = 16;
        let mut t = [[[0.0f64; 9]; 9]; 9];
        for &(mu, w) in &nodes {
            let s = (1.0 - mu * mu).max(0.0).sqrt();
            for b in 0..steps {
                let phi = (b as f64 + 0.5) * std::f64::consts::TAU / steps as f64;
                let d = [(s * phi.cos()) as f32, (s * phi.sin()) as f32, mu as f32];
                let y = sh_basis(d).map(f64::from);
                let wt = w * std::f64::consts::TAU / steps as f64;
                for i in 0..9 {
                    for j in 0..9 {
                        for k in 0..9 {
                            t[i][j][k] += wt * y[i] * y[j] * y[k];
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        for (i, a) in t.iter().enumerate() {
            for (j, b) in a.iter().enumerate() {
                for (k, &v) in b.iter().enumerate() {
                    if v.abs() > 1e-5 {
                        out.push((i, j, k, v as f32));
                    }
                }
            }
        }
        out
    })
}

/// Nodes and weights of the `n`-point Gauss-Legendre rule on [-1, 1].
fn gauss_legendre(n: usize) -> Vec<(f64, f64)> {
    (1..=n)
        .map(|i| {
            let mut x = (std::f64::consts::PI * (i as f64 - 0.25) / (n as f64 + 0.5)).cos();
            let mut dp = 1.0;
            for _ in 0..50 {
                let (mut p0, mut p1) = (1.0, x);
                for k in 2..=n {
                    let k = k as f64;
                    let p2 = ((2.0 * k - 1.0) * x * p1 - (k - 1.0) * p0) / k;
                    p0 = p1;
                    p1 = p2;
                }
                dp = n as f64 * (x * p1 - p0) / (x * x - 1.0);
                let dx = p1 / dp;
                x -= dx;
                if dx.abs() < 1e-15 {
                    break;
                }
            }
            (x, 2.0 / ((1.0 - x * x) * dp * dp))
        })
        .collect()
}

/// The product of two functions given as harmonics, projected back onto the nine.
pub fn sh_product(a: &[f32; 9], b: &[f32; 9]) -> [f32; 9] {
    let mut out = [0.0; 9];
    for &(i, j, k, t) in sh_triple() {
        out[k] += t * a[i] * b[j];
    }
    out
}

/// The clamped cosine round `n` as harmonics: `A_l Y_lm(n)`.
pub fn cosine_lobe(n: [f32; 3]) -> [f32; 9] {
    let y = sh_basis(n);
    std::array::from_fn(|k| COSINE_BAND[BAND[k]] * y[k])
}

/// The irradiance of a surface facing `n` from a sky of `radiance` (harmonics per colour
/// channel) seen through `visibility` (harmonics): `∫ L V max(n.w, 0) dw`, the product of sky
/// and visibility projected onto the nine and convolved with the cosine.
pub fn sky_irradiance(radiance: &[[f32; 3]; 9], visibility: &[f32; 9], n: [f32; 3]) -> [f32; 3] {
    let w = sh_product(visibility, &cosine_lobe(n));
    let mut e = [0.0; 3];
    for (l, wi) in radiance.iter().zip(w) {
        for c in 0..3 {
            e[c] += l[c] * wi;
        }
    }
    e.map(|v| v.max(0.0))
}

/// The cosine-weighted share of the sky a surface facing `n` sees (0..1): its irradiance under
/// a uniform sky of unit radiance, over pi. 1 under the open sky.
pub fn sky_view_of(visibility: &[f32; 9], n: [f32; 3]) -> f32 {
    let c = cosine_lobe(n);
    (visibility.iter().zip(c).map(|(v, c)| v * c).sum::<f32>() / PI).clamp(0.0, 1.0)
}

/// The mean visibility over a cone of half-angle `half_angle` round `d` (Funk-Hecke with the
/// cap's zonal harmonics: band factors 1, (1 + c) / 2 and c (1 + c) / 2 for c the cosine of the
/// half-angle): the specular occlusion of a glossy reflection along `d`.
pub fn cone_visibility(visibility: &[f32; 9], d: [f32; 3], half_angle: f32) -> f32 {
    let c = half_angle.cos();
    let band = [1.0, 0.5 * (1.0 + c), 0.5 * c * (1.0 + c)];
    let y = sh_basis(d);
    (0..9)
        .map(|k| band[BAND[k]] * visibility[k] * y[k])
        .sum::<f32>()
        .clamp(0.0, 1.0)
}

/// `n` directions spread evenly over the sphere (a Fibonacci sphere).
fn sphere(n: usize) -> Vec<[f32; 3]> {
    let golden = PI * (3.0 - 5f32.sqrt());
    (0..n)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
            let r = (1.0 - y * y).max(0.0).sqrt();
            let phi = i as f32 * golden;
            [r * phi.cos(), y, r * phi.sin()]
        })
        .collect()
}

// ---- the scene -------------------------------------------------------------------------------

/// A crown of leaves: an ellipsoid a ray crosses with transmittance `exp(-extinction * chord)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Canopy {
    pub center: [f32; 3],
    /// Half the size along x, y and z.
    pub radii: [f32; 3],
    /// Per metre of leaves crossed.
    pub extinction: f32,
}

impl Canopy {
    /// The metres of the ray `p + t d` (unit `d`, `0 <= t <= metres`) inside the crown.
    fn chord(&self, p: [f32; 3], d: [f32; 3], metres: f32) -> f32 {
        let o: [f32; 3] = std::array::from_fn(|k| (p[k] - self.center[k]) / self.radii[k]);
        let e: [f32; 3] = std::array::from_fn(|k| d[k] / self.radii[k]);
        let a: f32 = e.iter().map(|v| v * v).sum();
        let b: f32 = (0..3).map(|k| o[k] * e[k]).sum();
        let c: f32 = o.iter().map(|v| v * v).sum::<f32>() - 1.0;
        let disc = b * b - a * c;
        if disc <= 0.0 || a <= 0.0 {
            return 0.0;
        }
        let s = disc.sqrt();
        let (t0, t1) = ((-b - s) / a, (-b + s) / a);
        (t1.min(metres) - t0.max(0.0)).max(0.0)
    }

    fn reach(&self) -> f32 {
        self.radii[0].max(self.radii[1]).max(self.radii[2])
    }
}

/// The leaf area index (one-sided leaf area per area of ground under the crown) of a species in
/// full leaf, from published ranges for open-grown trees and shrubs (Bréda, "Ground-based
/// measurements of leaf area index", J. Exp. Bot. 54, 2003: broadleaved stands 3-6, birch and
/// cherry towards the light end, beech, hornbeam and lime towards the dense end; conifers 5-9 but
/// clumped). Unknown species take their grower's.
pub fn leaf_area_index(asset: &str) -> f32 {
    let id = asset.strip_prefix("flora:").unwrap_or(asset);
    let known = [
        ("betula-pendula", 2.5),
        ("robinia-pseudoacacia", 2.5),
        ("olea-europaea", 2.5),
        ("prunus-serrulata", 3.0),
        ("malus-domestica", 3.0),
        ("pyrus-communis", 3.0),
        ("prunus-domestica", 3.0),
        ("sorbus-aucuparia", 3.0),
        ("larix-decidua", 3.0),
        ("pinus-sylvestris", 3.0),
        ("fraxinus-excelsior", 3.5),
        ("salix-sepulcralis", 3.5),
        ("salix-alba", 3.5),
        ("populus-nigra-italica", 3.5),
        ("ginkgo-biloba", 3.5),
        ("acer-campestre", 4.5),
        ("quercus-robur", 4.5),
        ("castanea-sativa", 5.0),
        ("acer-platanoides", 5.5),
        ("aesculus-hippocastanum", 5.5),
        ("tilia-cordata", 6.0),
        ("carpinus-betulus", 6.0),
        ("fagus-sylvatica", 6.5),
        ("taxus-baccata", 7.0),
        ("picea-abies", 7.0),
        ("abies-nordmanniana", 7.0),
    ];
    if let Some(&(_, lai)) = known.iter().find(|k| k.0 == id) {
        return lai;
    }
    use crate::growth::Grower;
    match crate::growth::growth_of(asset, None).grower {
        Grower::Tree => 4.0,
        Grower::Conifer => 6.0,
        Grower::Palm => 2.0,
        Grower::Shrub => 3.5,
        // A clipped hedge is a shell of leaves kept dense by the shears.
        Grower::Clipped => 7.0,
        _ => 2.0,
    }
}

/// The wood of a leafless deciduous crown, as an area index (branches and twigs; Bréda 2003).
const WOOD_AREA_INDEX: f32 = 0.7;

/// The extinction per metre of a crown of half-height `ry` (m) for its species (`asset`) with
/// `leaves` of its leaves on (`sun_hours::leaf_share`): `G Ω LAI / depth`, with the leaves
/// spread over directions evenly (G = 0.5: Ross, "The Radiation Regime and Architecture of Plant
/// Stands", 1981), clumped into shoots (Ω 0.6 for conifers, 0.8 otherwise) and the mean vertical
/// chord of the ellipsoid (4/3 of its half-height) as the depth the leaf area is spread over.
pub fn crown_extinction(asset: &str, ry: f32, leaves: f32) -> f32 {
    let conifer = matches!(
        crate::growth::growth_of(asset, None).grower,
        crate::growth::Grower::Conifer
    );
    let clumping = if conifer { 0.6 } else { 0.8 };
    let area = leaf_area_index(asset) * leaves + WOOD_AREA_INDEX;
    0.5 * clumping * area / (4.0 / 3.0 * ry.max(0.1))
}

/// The height of the ground over a grid (terrain and terraces), or a level plane.
#[derive(Clone, Debug, PartialEq)]
pub struct Ground {
    /// The first sample's (x, z).
    pub origin: [f32; 2],
    pub cell: f32,
    /// Samples along x and z; zero: the plane at `base` everywhere.
    pub dims: [u32; 2],
    /// x fastest.
    pub heights: Vec<f32>,
    /// The height outside the grid's samples where there are none.
    pub base: f32,
}

impl Ground {
    /// Level ground at `y`.
    pub fn flat(y: f32) -> Self {
        Self {
            origin: [0.0; 2],
            cell: 1.0,
            dims: [0, 0],
            heights: Vec::new(),
            base: y,
        }
    }

    /// The ground sampled from `height` every `cell` metres over `lo..hi` (x, z).
    pub fn sample(lo: [f32; 2], hi: [f32; 2], cell: f32, height: impl Fn(f32, f32) -> f32) -> Self {
        let dims: [u32; 2] = std::array::from_fn(|k| ((hi[k] - lo[k]) / cell).ceil() as u32 + 1);
        let mut heights = Vec::with_capacity((dims[0] * dims[1]) as usize);
        for z in 0..dims[1] {
            for x in 0..dims[0] {
                heights.push(height(lo[0] + cell * x as f32, lo[1] + cell * z as f32));
            }
        }
        let base = heights.iter().copied().fold(f32::INFINITY, f32::min);
        Self {
            origin: lo,
            cell,
            dims,
            heights,
            base: if base.is_finite() { base } else { 0.0 },
        }
    }

    /// The height at `(x, z)`, interpolated, the grid's edge carried on outside it.
    pub fn height(&self, x: f32, z: f32) -> f32 {
        if self.dims[0] == 0 || self.dims[1] == 0 {
            return self.base;
        }
        let fx = ((x - self.origin[0]) / self.cell).clamp(0.0, (self.dims[0] - 1) as f32);
        let fz = ((z - self.origin[1]) / self.cell).clamp(0.0, (self.dims[1] - 1) as f32);
        let (x0, z0) = (fx.floor() as u32, fz.floor() as u32);
        let (x1, z1) = (
            (x0 + 1).min(self.dims[0] - 1),
            (z0 + 1).min(self.dims[1] - 1),
        );
        let (tx, tz) = (fx - x0 as f32, fz - z0 as f32);
        let h = |x: u32, z: u32| self.heights[(x + self.dims[0] * z) as usize];
        let a = h(x0, z0) + (h(x1, z0) - h(x0, z0)) * tx;
        let b = h(x0, z1) + (h(x1, z1) - h(x0, z1)) * tx;
        a + (b - a) * tz
    }

    fn range(&self) -> (f32, f32) {
        self.heights
            .iter()
            .fold((self.base, self.base), |(lo, hi), &h| {
                (lo.min(h), hi.max(h))
            })
    }
}

/// What blocks the sky: world triangles (buildings, shapes, models; glass and the ground's own
/// areas left out), the crowns, and the ground.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyScene {
    pub triangles: Vec<[[f32; 3]; 3]>,
    pub crowns: Vec<Canopy>,
    pub ground: Ground,
}

impl SkyScene {
    /// A hash of everything a bake depends on: the same key, the same bake.
    pub fn key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut f = |v: f32| v.to_bits().hash(&mut h);
        self.triangles
            .iter()
            .flatten()
            .flatten()
            .for_each(|&v| f(v));
        for c in &self.crowns {
            c.center.iter().chain(&c.radii).for_each(|&v| f(v));
            f(c.extinction);
        }
        let g = &self.ground;
        g.origin.iter().chain(&g.heights).for_each(|&v| f(v));
        f(g.cell);
        f(g.base);
        g.dims.hash(&mut h);
        h.finish()
    }
}

/// The scene ready for rays: the scene and its triangles binned into columns (`Bins`, made
/// once per bake: the binning costs as much as a few hundred probes).
struct Tracer<'a> {
    scene: &'a SkyScene,
    bins: &'a Bins,
}

impl std::ops::Deref for Tracer<'_> {
    type Target = Bins;
    fn deref(&self) -> &Bins {
        self.bins
    }
}

/// The triangles of a scene binned into columns over the ground.
struct Bins {
    /// The columns: origin (x, z), size, count.
    lo: [f32; 2],
    col: f32,
    cols: [i32; 2],
    /// Per column: the start of its triangles in `ids`, and its highest point.
    starts: Vec<u32>,
    ids: Vec<u32>,
    tops: Vec<f32>,
    /// The highest and lowest points anything reaches.
    top: f32,
    bottom: f32,
    /// The ground has relief worth tracing (else a level plane).
    relief: bool,
    ground_top: f32,
}

/// What a ray finds.
struct Ray {
    /// The share of the sky (or the open ground) seen along it.
    visibility: f32,
    /// It met a building, a wall or the terrain (not the open ground).
    solid: bool,
    /// The reflectance-weighted share it lost: what the occluders give back.
    lost_albedo: f32,
}

impl Bins {
    fn new(scene: &SkyScene, lo: [f32; 2], hi: [f32; 2]) -> Self {
        let col = ((hi[0] - lo[0]).max(hi[1] - lo[1]) / 256.0).max(1.0);
        let cols: [i32; 2] = std::array::from_fn(|k| ((hi[k] - lo[k]) / col).ceil() as i32 + 1);
        let n = (cols[0] * cols[1]) as usize;
        let mut bins: Vec<Vec<u32>> = vec![Vec::new(); n];
        let mut tops = vec![f32::NEG_INFINITY; n];
        let (ground_lo, ground_hi) = scene.ground.range();
        let (mut top, mut bottom) = (ground_hi, ground_lo);
        for (i, t) in scene.triangles.iter().enumerate() {
            let mut a = [f32::INFINITY; 3];
            let mut b = [f32::NEG_INFINITY; 3];
            for p in t {
                for k in 0..3 {
                    a[k] = a[k].min(p[k]);
                    b[k] = b[k].max(p[k]);
                }
            }
            top = top.max(b[1]);
            bottom = bottom.min(a[1]);
            let c0 = [
                ((a[0] - lo[0]) / col).floor() as i32,
                ((a[2] - lo[1]) / col).floor() as i32,
            ];
            let c1 = [
                ((b[0] - lo[0]) / col).floor() as i32,
                ((b[2] - lo[1]) / col).floor() as i32,
            ];
            for cz in c0[1].max(0)..=c1[1].min(cols[1] - 1) {
                for cx in c0[0].max(0)..=c1[0].min(cols[0] - 1) {
                    let j = (cx + cols[0] * cz) as usize;
                    bins[j].push(i as u32);
                    tops[j] = tops[j].max(b[1]);
                }
            }
        }
        for c in &scene.crowns {
            top = top.max(c.center[1] + c.radii[1]);
            bottom = bottom.min(c.center[1] - c.radii[1]);
        }
        let mut starts = Vec::with_capacity(n + 1);
        let mut ids = Vec::new();
        for b in &bins {
            starts.push(ids.len() as u32);
            ids.extend_from_slice(b);
        }
        starts.push(ids.len() as u32);
        Self {
            lo,
            col,
            cols,
            starts,
            ids,
            tops,
            top: top + 0.01,
            bottom: bottom - 0.01,
            relief: ground_hi - ground_lo > 0.2,
            ground_top: ground_hi,
        }
    }
}

impl Tracer<'_> {
    /// The nearest triangle along the ray within `limit`: distance and the triangle's normal.
    fn triangle_hit(&self, p: [f32; 3], d: [f32; 3], limit: f32) -> Option<(f32, [f32; 3])> {
        // 2D DDA through the columns (Amanatides and Woo, "A Fast Voxel Traversal Algorithm
        // for Ray Tracing", Eurographics 1987), clipped to the grid.
        let (px, pz) = (p[0] - self.lo[0], p[2] - self.lo[1]);
        let size = [
            self.cols[0] as f32 * self.col,
            self.cols[1] as f32 * self.col,
        ];
        let (mut t0, mut t1) = (0.0f32, limit);
        for (o, dk, s) in [(px, d[0], size[0]), (pz, d[2], size[1])] {
            if dk.abs() < 1e-9 {
                if o < 0.0 || o >= s {
                    return None;
                }
            } else {
                let (a, b) = ((0.0 - o) / dk, (s - o) / dk);
                t0 = t0.max(a.min(b));
                t1 = t1.min(a.max(b));
            }
        }
        if t0 > t1 {
            return None;
        }
        let start = [px + d[0] * t0, pz + d[2] * t0];
        let mut c = [
            ((start[0] / self.col).floor() as i32).clamp(0, self.cols[0] - 1),
            ((start[1] / self.col).floor() as i32).clamp(0, self.cols[1] - 1),
        ];
        let dir = [d[0], d[2]];
        let step: [i32; 2] = std::array::from_fn(|k| if dir[k] > 0.0 { 1 } else { -1 });
        let delta: [f32; 2] = std::array::from_fn(|k| {
            if dir[k].abs() < 1e-9 {
                f32::INFINITY
            } else {
                self.col / dir[k].abs()
            }
        });
        let o = [px, pz];
        let mut next: [f32; 2] = std::array::from_fn(|k| {
            if dir[k].abs() < 1e-9 {
                f32::INFINITY
            } else {
                let edge = (c[k] + i32::from(dir[k] > 0.0)) as f32 * self.col;
                (edge - o[k]) / dir[k]
            }
        });
        let mut enter = t0;
        loop {
            let exit = next[0].min(next[1]).min(t1);
            let j = (c[0] + self.cols[0] * c[1]) as usize;
            // The ray's lowest point over this column, against the column's top.
            let low = p[1] + d[1] * if d[1] >= 0.0 { enter } else { exit };
            if low <= self.tops[j] {
                let mut best: Option<(f32, [f32; 3])> = None;
                for &i in &self.ids[self.starts[j] as usize..self.starts[j + 1] as usize] {
                    if let Some(h) = hit_triangle(&self.scene.triangles[i as usize], p, d)
                        && h.0 <= exit + 1e-4
                        && best.is_none_or(|b| h.0 < b.0)
                    {
                        best = Some(h);
                    }
                }
                if best.is_some() {
                    return best;
                }
            }
            if exit >= t1 {
                return None;
            }
            // Out of reach of everything above or below.
            let y = p[1] + d[1] * exit;
            if (d[1] > 0.0 && y > self.top) || (d[1] < 0.0 && y < self.bottom) {
                return None;
            }
            let k = usize::from(next[0] > next[1]);
            c[k] += step[k];
            if c[k] < 0 || c[k] >= self.cols[k] {
                return None;
            }
            enter = next[k];
            next[k] += delta[k];
        }
    }

    /// The distance to the ground along the ray (the terrain's relief traced; a level plane
    /// solved), within `limit`.
    fn ground_hit(&self, p: [f32; 3], d: [f32; 3], limit: f32) -> Option<f32> {
        let g = &self.scene.ground;
        if !self.relief {
            if d[1] >= -1e-6 {
                return None;
            }
            let t = (p[1] - g.height(p[0], p[2])) / -d[1];
            return (t >= 0.0 && t <= limit).then_some(t);
        }
        let step = (g.cell * 0.5).max(0.1);
        let mut prev = 0.0f32;
        let mut t = step;
        while t <= limit {
            let q = [p[0] + d[0] * t, p[1] + d[1] * t, p[2] + d[2] * t];
            if d[1] >= 0.0 && q[1] > self.ground_top {
                return None;
            }
            if q[1] < g.height(q[0], q[2]) {
                // Halve the step a few times to the crossing.
                let (mut a, mut b) = (prev, t);
                for _ in 0..6 {
                    let m = 0.5 * (a + b);
                    let r = [p[0] + d[0] * m, p[1] + d[1] * m, p[2] + d[2] * m];
                    if r[1] < g.height(r[0], r[2]) {
                        b = m;
                    } else {
                        a = m;
                    }
                }
                return Some(b);
            }
            prev = t;
            t += step;
        }
        None
    }

    /// How far a ray can go: out of the box of everything that blocks.
    fn reach(&self, p: [f32; 3], d: [f32; 3]) -> f32 {
        let span = (self.cols[0].max(self.cols[1]) as f32 * self.col) * 1.5 + 50.0;
        if d[1] > 1e-6 {
            ((self.top.max(self.ground_top) - p[1]) / d[1]).clamp(0.0, span)
        } else {
            span
        }
    }

    /// One ray from `p` along unit `d`, the crowns in `near` (indexes into the scene's crowns
    /// with the cone each fills seen from `p`: unit axis and the cosine of its half-angle).
    fn ray(&self, p: [f32; 3], d: [f32; 3], near: &[(usize, [f32; 3], f32)]) -> Ray {
        let limit = self.reach(p, d);
        let tri = self.triangle_hit(p, d, limit);
        let ground = self.ground_hit(p, d, tri.map_or(limit, |h| h.0));
        // What it ends on: a building or wall, the ground, the top of something seen from above,
        // or nothing.
        let (end, solid) = match (tri, ground) {
            (_, Some(t)) => (t, d[1] >= 0.0),
            (Some((t, n)), None) => {
                let up = if n[1] < 0.0 { -n[1] } else { n[1] };
                (t, !(d[1] < 0.0 && up > 0.7))
            }
            (None, None) => (limit, false),
        };
        if solid {
            return Ray {
                visibility: 0.0,
                solid: true,
                lost_albedo: SOLID_ALBEDO,
            };
        }
        let mut depth = 0.0;
        for &(i, axis, cos) in near {
            if axis[0] * d[0] + axis[1] * d[1] + axis[2] * d[2] < cos {
                continue;
            }
            let c = &self.scene.crowns[i];
            depth += c.extinction * c.chord(p, d, end);
        }
        let t = (-depth).exp();
        Ray {
            visibility: t,
            solid: false,
            lost_albedo: (1.0 - t) * LEAF_ALBEDO,
        }
    }

    /// The crowns a ray from `p` may cross, with their cones (`ray`). A crown `p` is inside is
    /// left out: a point among a crown's leaves is lit through the crown's own leaves by the
    /// renderer's per-leaf crown occlusion (finer than a metre's grid), so the grid holds what
    /// reaches the crown from outside, and a thin hedge's own probes do not darken its faces.
    fn near(&self, p: [f32; 3]) -> Vec<(usize, [f32; 3], f32)> {
        self.scene
            .crowns
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                (0..3)
                    .map(|k| ((p[k] - c.center[k]) / c.radii[k]).powi(2))
                    .sum::<f32>()
                    >= 1.0
            })
            .map(|(i, c)| {
                let v: [f32; 3] = std::array::from_fn(|k| c.center[k] - p[k]);
                let dist = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                let r = c.reach();
                if dist <= r * 1.001 {
                    return (i, [0.0, 1.0, 0.0], -2.0);
                }
                let s = r / dist;
                (i, v.map(|x| x / dist), (1.0 - s * s).max(0.0).sqrt())
            })
            .collect()
    }

    /// One probe at `p`: its visibility harmonics, whether it is outdoors and the mean
    /// reflectance of what it sees instead of sky. `turn`: its own angle round the vertical.
    fn probe(&self, p: [f32; 3], dirs: &[[f32; 3]], turn: f32) -> ([f32; 9], bool, f32) {
        let near = self.near(p);
        let (s, c) = turn.sin_cos();
        let mut sh = [0.0f32; 9];
        let (mut up, mut up_open, mut lost, mut albedo) = (0, 0, 0.0f32, 0.0f32);
        for d in dirs {
            let d = [c * d[0] - s * d[2], d[1], s * d[0] + c * d[2]];
            let r = self.ray(p, d, &near);
            let y = sh_basis(d);
            for k in 0..9 {
                sh[k] += r.visibility * y[k];
            }
            if d[1] > 0.05 {
                up += 1;
                up_open += i32::from(!r.solid);
            }
            lost += 1.0 - r.visibility;
            albedo += r.lost_albedo;
        }
        let w = 4.0 * PI / dirs.len() as f32;
        let outdoors = up == 0 || up_open as f32 >= INDOOR_SHARE * up as f32;
        let albedo = if lost > 1e-3 {
            albedo / lost
        } else {
            SOLID_ALBEDO
        };
        (sh.map(|v| v * w), outdoors, albedo)
    }
}

/// Möller-Trumbore, both sides: the distance and the (unnormalised) normal.
fn hit_triangle(t: &[[f32; 3]; 3], p: [f32; 3], d: [f32; 3]) -> Option<(f32, [f32; 3])> {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (e1, e2) = (sub(t[1], t[0]), sub(t[2], t[0]));
    let h = cross(d, e2);
    let a = dot(e1, h);
    if a.abs() < 1e-10 {
        return None;
    }
    let f = 1.0 / a;
    let s = sub(p, t[0]);
    let u = f * dot(s, h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = f * dot(d, q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let dist = f * dot(e2, q);
    if dist <= 1e-4 {
        return None;
    }
    let n = cross(e1, e2);
    let len = dot(n, n).sqrt().max(1e-12);
    Some((dist, n.map(|v| v / len)))
}

/// The cosine-weighted share of the sky a surface at `p` facing `n` sees, traced directly with
/// `rays` rays spread evenly over the projected disc (so the cosine weighting is in the spread):
/// the sky view factor of `query.sky_view`, 1 under the open sky.
pub fn sky_view_at(scene: &SkyScene, p: [f32; 3], n: [f32; 3], rays: usize) -> f32 {
    let r = PAD_M + 60.0;
    let bins = Bins::new(scene, [p[0] - r, p[2] - r], [p[0] + r, p[2] + r]);
    let tracer = Tracer {
        scene,
        bins: &bins,
    };
    let near = tracer.near(p);
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
    let n = n.map(|v| v / len);
    // A frame round n.
    let t = if n[0].abs() > 0.9 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let a = {
        let c = [
            t[1] * n[2] - t[2] * n[1],
            t[2] * n[0] - t[0] * n[2],
            t[0] * n[1] - t[1] * n[0],
        ];
        let l = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
        c.map(|v| v / l)
    };
    let b = [
        n[1] * a[2] - n[2] * a[1],
        n[2] * a[0] - n[0] * a[2],
        n[0] * a[1] - n[1] * a[0],
    ];
    let golden = PI * (3.0 - 5f32.sqrt());
    let mut sum = 0.0;
    for i in 0..rays {
        let r2 = (i as f32 + 0.5) / rays as f32;
        let (s, c) = (i as f32 * golden).sin_cos();
        let (x, y, z) = (r2.sqrt() * c, r2.sqrt() * s, (1.0 - r2).max(0.0).sqrt());
        let d: [f32; 3] = std::array::from_fn(|k| a[k] * x + b[k] * y + n[k] * z);
        // Only the sky counts here: the open ground below the horizon is not sky.
        let ray = tracer.ray(p, d, &near);
        if d[1] >= 0.0 {
            sum += ray.visibility;
        }
    }
    sum / rays as f32
}

// ---- the grid --------------------------------------------------------------------------------

/// The baked sky visibility: probe `(x, k, z)` stands at `(origin.x + cell x, ground + layer
/// height k, origin.z + cell z)`, with layer height `lift + height (k / (layers - 1))^2` over the
/// ground of its column.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyVisibility {
    /// (x, z) of the first column.
    pub origin: [f32; 2],
    pub cell: f32,
    /// Columns along x, layers, columns along z.
    pub dims: [u32; 3],
    pub lift: f32,
    pub height: f32,
    /// Per column (x fastest): the ground's height.
    pub ground: Vec<f32>,
    /// Per probe (x fastest, then the layer, then z): the visibility harmonics.
    pub sh: Vec<[f32; 9]>,
    /// Per probe: 1 outdoors, 0 indoors or inside a wall (left out of the interpolation).
    pub valid: Vec<f32>,
    /// Per probe: the mean reflectance of what it sees instead of sky.
    pub albedo: Vec<f32>,
}

impl SkyVisibility {
    fn layer_height(&self, k: f32) -> f32 {
        let t = k / (self.dims[1] - 1).max(1) as f32;
        self.lift + self.height * t * t
    }

    fn layer_of(&self, h: f32) -> f32 {
        (self.dims[1] - 1) as f32 * ((h - self.lift).max(0.0) / self.height).sqrt()
    }

    /// The ground under `(x, z)`, interpolated between the columns.
    pub fn ground_at(&self, x: f32, z: f32) -> f32 {
        let fx = ((x - self.origin[0]) / self.cell).clamp(0.0, (self.dims[0] - 1) as f32);
        let fz = ((z - self.origin[1]) / self.cell).clamp(0.0, (self.dims[2] - 1) as f32);
        let (x0, z0) = (fx.floor() as u32, fz.floor() as u32);
        let (x1, z1) = (
            (x0 + 1).min(self.dims[0] - 1),
            (z0 + 1).min(self.dims[2] - 1),
        );
        let (tx, tz) = (fx - x0 as f32, fz - z0 as f32);
        let g = |x: u32, z: u32| self.ground[(x + self.dims[0] * z) as usize];
        let a = g(x0, z0) + (g(x1, z0) - g(x0, z0)) * tx;
        let b = g(x0, z1) + (g(x1, z1) - g(x0, z1)) * tx;
        a + (b - a) * tz
    }

    /// The harmonics at `p`, weighted trilinearly over the outdoor probes round it (as the
    /// renderer reads them), with the weight of those probes and their occluders' reflectance;
    /// `None` outside the grid or where only indoor probes are round.
    pub fn sample(&self, p: [f32; 3]) -> Option<([f32; 9], f32, f32)> {
        let [nx, nl, nz] = self.dims;
        let fx = (p[0] - self.origin[0]) / self.cell;
        let fz = (p[2] - self.origin[1]) / self.cell;
        if fx < 0.0 || fz < 0.0 || fx > (nx - 1) as f32 || fz > (nz - 1) as f32 {
            return None;
        }
        let fk = self.layer_of(p[1] - self.ground_at(p[0], p[2]));
        if fk > (nl - 1) as f32 {
            return None;
        }
        let (x0, k0, z0) = (fx.floor() as u32, fk.floor() as u32, fz.floor() as u32);
        let f = [fx - x0 as f32, fk - k0 as f32, fz - z0 as f32];
        let (mut sh, mut w_sum, mut albedo) = ([0.0f32; 9], 0.0f32, 0.0f32);
        for c in 0..8u32 {
            let o = [c & 1, (c >> 1) & 1, c >> 2];
            let at = [
                (x0 + o[0]).min(nx - 1),
                (k0 + o[1]).min(nl - 1),
                (z0 + o[2]).min(nz - 1),
            ];
            let tri: f32 = (0..3)
                .map(|k| if o[k] == 1 { f[k] } else { 1.0 - f[k] })
                .product();
            let i = (at[0] + nx * (at[1] + nl * at[2])) as usize;
            let w = tri * self.valid[i];
            for k in 0..9 {
                sh[k] += w * self.sh[i][k];
            }
            albedo += w * self.albedo[i];
            w_sum += w;
        }
        (w_sum > 1e-4).then(|| (sh.map(|v| v / w_sum), w_sum, albedo / w_sum))
    }

    /// The sky view of a surface at `p` facing `n`, read from the grid as the renderer reads it
    /// (lifted half a cell, at most 0.6 m, off the surface): 1 outside it.
    pub fn sky_view(&self, p: [f32; 3], n: [f32; 3]) -> f32 {
        let lift = (self.cell * 0.5).min(0.6);
        let q: [f32; 3] = std::array::from_fn(|k| p[k] + n[k] * lift);
        self.sample(q).map_or(1.0, |(sh, _, _)| sky_view_of(&sh, n))
    }

    /// The probes as the renderer's 3D texture: three tiles side by side along x (coefficients
    /// 0-3, 4-7, and 8 with the weight, the ground and the reflectance), the coefficients and the
    /// reflectance multiplied by the weight so a filtered read divided by the filtered weight is
    /// the interpolation over the outdoor probes (common.wgsl `sky_occ`). x fastest, then the
    /// layer, then z; width three times the columns along x.
    pub fn texels(&self) -> Vec<[f32; 4]> {
        let [nx, nl, nz] = self.dims;
        let mut out = Vec::with_capacity((3 * nx * nl * nz) as usize);
        for z in 0..nz {
            for k in 0..nl {
                for tile in 0..3 {
                    for x in 0..nx {
                        let i = (x + nx * (k + nl * z)) as usize;
                        let (s, w) = (&self.sh[i], self.valid[i]);
                        out.push(match tile {
                            0 => [s[0] * w, s[1] * w, s[2] * w, s[3] * w],
                            1 => [s[4] * w, s[5] * w, s[6] * w, s[7] * w],
                            _ => [
                                s[8] * w,
                                w,
                                self.ground[(x + nx * z) as usize],
                                self.albedo[i] * w,
                            ],
                        });
                    }
                }
            }
        }
        out
    }
}

/// A bake under way, a few probes at a time: a host that must not stop (a window on the web)
/// calls [`SkyBake::step`] every frame; [`bake`] runs it at once, on all cores natively.
pub struct SkyBake {
    scene: SkyScene,
    grid: SkyVisibility,
    /// The scene's triangles binned for rays, once for the whole bake.
    bins: Bins,
    dirs: Vec<[f32; 3]>,
    next: usize,
}

impl SkyBake {
    /// The bake for `scene`; `None` when nothing blocks the sky.
    pub fn new(scene: SkyScene) -> Option<Self> {
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        let mut top = f32::NEG_INFINITY;
        let mut grow = |p: [f32; 3], r: [f32; 3]| {
            lo[0] = lo[0].min(p[0] - r[0]);
            lo[1] = lo[1].min(p[2] - r[2]);
            hi[0] = hi[0].max(p[0] + r[0]);
            hi[1] = hi[1].max(p[2] + r[2]);
            top = top.max(p[1] + r[1]);
        };
        scene
            .triangles
            .iter()
            .flatten()
            .for_each(|p| grow(*p, [0.0; 3]));
        scene.crowns.iter().for_each(|c| grow(c.center, c.radii));
        if !lo[0].is_finite() {
            return None;
        }
        let lo = [lo[0] - PAD_M, lo[1] - PAD_M];
        let hi = [hi[0] + PAD_M, hi[1] + PAD_M];
        let mut cell = CELL_M;
        let count = |c: f32| -> [u32; 2] {
            std::array::from_fn(|k| ((hi[k] - lo[k]) / c).ceil() as u32 + 1)
        };
        while count(cell).iter().map(|&n| n as usize).product::<usize>() * LAYERS as usize
            > MAX_PROBES
        {
            cell *= 1.25;
        }
        let [nx, nz] = count(cell);
        let ground: Vec<f32> = (0..nz)
            .flat_map(|z| (0..nx).map(move |x| (x, z)))
            .map(|(x, z)| {
                scene
                    .ground
                    .height(lo[0] + cell * x as f32, lo[1] + cell * z as f32)
            })
            .collect();
        let low = ground.iter().copied().fold(f32::INFINITY, f32::min);
        let height = (top - low + 1.0).clamp(3.0, 40.0);
        let n = (nx * LAYERS * nz) as usize;
        let grid = SkyVisibility {
            origin: lo,
            cell,
            dims: [nx, LAYERS, nz],
            lift: LIFT_M,
            height,
            ground,
            sh: Vec::with_capacity(n),
            valid: Vec::with_capacity(n),
            albedo: Vec::with_capacity(n),
        };
        let bins = Bins::new(&scene, lo, hi);
        Some(Self {
            scene,
            grid,
            bins,
            dirs: sphere(RAYS),
            next: 0,
        })
    }

    /// Probes in the grid, and how many are done.
    pub fn progress(&self) -> (usize, usize) {
        let [nx, nl, nz] = self.grid.dims;
        (self.next, (nx * nl * nz) as usize)
    }

    fn tracer(&self) -> Tracer<'_> {
        Tracer {
            scene: &self.scene,
            bins: &self.bins,
        }
    }

    /// Probe `i` of the grid.
    fn probe(&self, tracer: &Tracer, i: usize) -> ([f32; 9], bool, f32) {
        let g = &self.grid;
        let [nx, nl, _] = g.dims;
        let (x, k, z) = (i as u32 % nx, i as u32 / nx % nl, i as u32 / (nx * nl));
        let p = [
            g.origin[0] + g.cell * x as f32,
            g.ground[(x + nx * z) as usize] + g.layer_height(k as f32),
            g.origin[1] + g.cell * z as f32,
        ];
        let h =
            (x.wrapping_mul(73_856_093) ^ k.wrapping_mul(19_349_663) ^ z.wrapping_mul(83_492_791))
                % 1024;
        tracer.probe(p, &self.dirs, h as f32 * std::f32::consts::TAU / 1024.0)
    }

    fn push(&mut self, (sh, outdoors, albedo): ([f32; 9], bool, f32)) {
        self.grid.sh.push(sh);
        self.grid.valid.push(if outdoors { 1.0 } else { 0.0 });
        self.grid.albedo.push(albedo);
    }

    /// Does the next `n` probes; true when all are done.
    pub fn step(&mut self, n: usize) -> bool {
        let (done, total) = self.progress();
        let end = done.saturating_add(n).min(total);
        let made: Vec<_> = {
            let tracer = self.tracer();
            (done..end).map(|i| self.probe(&tracer, i)).collect()
        };
        made.into_iter().for_each(|m| self.push(m));
        self.next = end;
        self.next == total
    }

    /// The grid; probes not done yet count as indoors.
    pub fn finish(mut self) -> SkyVisibility {
        let (done, total) = self.progress();
        for _ in done..total {
            self.push(([0.0; 9], false, SOLID_ALBEDO));
        }
        self.grid
    }

    /// All the probes at once: natively shared out over the cores (the same numbers as
    /// stepping, probe by probe).
    pub fn run(mut self) -> SkyVisibility {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (done, total) = self.progress();
            let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
            let per = (total - done).div_ceil(threads.max(1)).max(1);
            let made: Vec<_> = {
                let this = &self;
                std::thread::scope(|scope| {
                    let jobs: Vec<_> = (done..total)
                        .step_by(per)
                        .map(|a| {
                            scope.spawn(move || {
                                let tracer = this.tracer();
                                (a..(a + per).min(total))
                                    .map(|i| this.probe(&tracer, i))
                                    .collect::<Vec<_>>()
                            })
                        })
                        .collect();
                    jobs.into_iter()
                        .flat_map(|j| j.join().expect("a sky visibility thread panicked"))
                        .collect()
                })
            };
            made.into_iter().for_each(|m| self.push(m));
            self.next = total;
        }
        #[cfg(target_arch = "wasm32")]
        while !self.step(1024) {}
        self.finish()
    }
}

// ---- as floats (to a web worker and back) ---------------------------------------------------

/// Reads floats off the front of a slice.
struct Floats<'a>(&'a [f32]);

impl Floats<'_> {
    fn take(&mut self, n: usize) -> Option<&[f32]> {
        let (a, b) = self.0.split_at_checked(n)?;
        self.0 = b;
        Some(a)
    }
    fn one(&mut self) -> Option<f32> {
        Some(self.take(1)?[0])
    }
    /// A count written as a float (exact below 2^24).
    fn count(&mut self) -> Option<usize> {
        let v = self.one()?;
        (v >= 0.0 && v.fract() == 0.0).then_some(v as usize)
    }
    fn arr<const N: usize>(&mut self) -> Option<[f32; N]> {
        self.take(N)?.try_into().ok()
    }
}

impl SkyScene {
    /// The scene as floats, for a web worker (`from_floats`).
    pub fn to_floats(&self) -> Vec<f32> {
        let g = &self.ground;
        let mut out = vec![self.triangles.len() as f32];
        out.extend(self.triangles.as_flattened().as_flattened());
        out.push(self.crowns.len() as f32);
        for c in &self.crowns {
            out.extend(c.center.into_iter().chain(c.radii).chain([c.extinction]));
        }
        out.extend(g.origin);
        out.extend([g.cell, g.dims[0] as f32, g.dims[1] as f32, g.base]);
        out.push(g.heights.len() as f32);
        out.extend(&g.heights);
        out
    }

    pub fn from_floats(f: &[f32]) -> Option<Self> {
        let mut r = Floats(f);
        let n = r.count()?;
        let triangles = r
            .take(n.checked_mul(9)?)?
            .as_chunks::<9>()
            .0
            .iter()
            .map(|t| std::array::from_fn(|v| std::array::from_fn(|k| t[3 * v + k])))
            .collect();
        let n = r.count()?;
        let crowns = (0..n)
            .map(|_| {
                let c = r.arr::<7>()?;
                Some(Canopy {
                    center: [c[0], c[1], c[2]],
                    radii: [c[3], c[4], c[5]],
                    extinction: c[6],
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let origin = r.arr::<2>()?;
        let [cell, dx, dz, base] = r.arr::<4>()?;
        let n = r.count()?;
        let heights = r.take(n)?.to_vec();
        r.0.is_empty().then_some(Self {
            triangles,
            crowns,
            ground: Ground {
                origin,
                cell,
                dims: [dx as u32, dz as u32],
                heights,
                base,
            },
        })
    }
}

impl SkyVisibility {
    /// The grid as floats, for a web worker (`from_floats`).
    pub fn to_floats(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(16 + self.ground.len() + self.sh.len() * 11);
        out.extend(self.origin);
        out.extend([self.cell, self.lift, self.height]);
        out.extend(self.dims.map(|d| d as f32));
        out.push(self.ground.len() as f32);
        out.extend(&self.ground);
        out.push(self.sh.len() as f32);
        out.extend(self.sh.as_flattened());
        out.extend(&self.valid);
        out.extend(&self.albedo);
        out
    }

    pub fn from_floats(f: &[f32]) -> Option<Self> {
        let mut r = Floats(f);
        let origin = r.arr::<2>()?;
        let [cell, lift, height] = r.arr::<3>()?;
        let dims = r.arr::<3>()?.map(|d| d as u32);
        let n = r.count()?;
        let ground = r.take(n)?.to_vec();
        let n = r.count()?;
        let sh = r.take(n.checked_mul(9)?)?.as_chunks::<9>().0.to_vec();
        let valid = r.take(n)?.to_vec();
        let albedo = r.take(n)?.to_vec();
        r.0.is_empty().then_some(Self {
            origin,
            cell,
            dims,
            lift,
            height,
            ground,
            sh,
            valid,
            albedo,
        })
    }
}

/// The sky visibility of `scene`, baked at once; `None` when nothing blocks the sky.
pub fn bake(scene: SkyScene) -> Option<SkyVisibility> {
    SkyBake::new(scene).map(SkyBake::run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) -> Vec<[[f32; 3]; 3]> {
        vec![[a, b, c], [a, c, d]]
    }

    /// A wall in the plane x = `x` from z0 to z1, `h` high.
    fn wall(x: f32, z0: f32, z1: f32, h: f32) -> Vec<[[f32; 3]; 3]> {
        quad([x, 0.0, z0], [x, 0.0, z1], [x, h, z1], [x, h, z0])
    }

    #[test]
    fn the_triple_product_is_the_projection_of_the_product() {
        // The constant's product with anything is that thing times the constant.
        let c = 2.0 * (4.0 * PI).sqrt(); // the function 2
        let f = [0.3, -0.2, 0.5, 0.1, 0.05, -0.4, 0.2, 0.0, 0.15];
        let mut one = [0.0; 9];
        one[0] = c;
        let p = sh_product(&one, &f);
        for k in 0..9 {
            assert!((p[k] - 2.0 * f[k]).abs() < 1e-4, "{p:?}");
        }
        // Symmetric in its two arguments, and ∫Y_0 Y_j Y_k = δ_jk / sqrt(4π).
        let g = [0.1, 0.4, -0.3, 0.2, 0.0, 0.1, -0.2, 0.3, 0.05];
        let (a, b) = (sh_product(&f, &g), sh_product(&g, &f));
        assert!((0..9).all(|k| (a[k] - b[k]).abs() < 1e-5));
        let dot: f32 = (0..9).map(|k| f[k] * g[k]).sum();
        assert!((a[0] - dot / (4.0 * PI).sqrt()).abs() < 1e-5);
        // The quadrature: Y_1 Y_1 Y_6 (y, y, the zonal band-2 round z) is -1/sqrt(20π) ...
        let t = sh_triple()
            .iter()
            .find(|t| (t.0, t.1, t.2) == (1, 1, 6))
            .unwrap()
            .3;
        assert!((t + 1.0 / (20.0 * PI).sqrt()).abs() < 1e-5, "{t}");
    }

    #[test]
    fn a_constant_sky_gives_the_visibility_times_the_radiance() {
        // Visibility of the upper half of the sphere (y > 0), projected numerically.
        let dirs = sphere(20_000);
        let mut v = [0.0f32; 9];
        for d in &dirs {
            if d[1] > 0.0 {
                let y = sh_basis(*d);
                for k in 0..9 {
                    v[k] += y[k] * 4.0 * PI / dirs.len() as f32;
                }
            }
        }
        // A sky of radiance 3 in every direction (red), 1 (green), 0.5 (blue).
        let l = [3.0, 1.0, 0.5];
        let mut radiance = [[0.0f32; 3]; 9];
        radiance[0] = l.map(|c| c * (4.0 * PI).sqrt());
        for n in [
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.6, 0.8, 0.0],
        ] {
            let e = sky_irradiance(&radiance, &v, n);
            let view = sky_view_of(&v, n);
            for c in 0..3 {
                assert!(
                    (e[c] - PI * l[c] * view).abs() < 1e-3 * l[c],
                    "{n:?} {e:?} {view}"
                );
            }
        }
        // Facing up under the upper half: all of it (the L2 cosine is close, not exact).
        assert!((sky_view_of(&v, [0.0, 1.0, 0.0]) - 1.0).abs() < 0.08);
        // The whole sphere visible: pi times the radiance, facing any way.
        let mut all = [0.0f32; 9];
        all[0] = (4.0 * PI).sqrt();
        let e = sky_irradiance(&radiance, &all, [0.3, -0.5, 0.81]);
        assert!((e[0] - PI * 3.0).abs() < 1e-3, "{e:?}");
        // A cone round any direction of the whole sphere sees all of it.
        assert!((cone_visibility(&all, [0.0, 0.0, 1.0], 0.3) - 1.0).abs() < 1e-4);
        // A cone pointing up under the upper half sees more than one pointing down.
        assert!(cone_visibility(&v, [0.0, 1.0, 0.0], 0.4) > 0.9);
        assert!(cone_visibility(&v, [0.0, -1.0, 0.0], 0.4) < 0.1);
    }

    #[test]
    fn an_open_field_sees_all_the_sky() {
        let scene = SkyScene {
            triangles: Vec::new(),
            crowns: Vec::new(),
            ground: Ground::flat(0.0),
        };
        assert!((sky_view_at(&scene, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0], 512) - 1.0).abs() < 1e-6);
        // Nothing to bake.
        assert!(bake(scene).is_none());
    }

    #[test]
    fn the_bottom_of_a_narrow_courtyard_sees_the_analytic_sky() {
        // Two 6 m walls 2 m apart, 200 m long: the floor's view of the slot of sky between
        // them is sin(atan(1 / 6)), the infinite canyon's (Oke, "Boundary Layer Climates",
        // 1987: cos of the walls' elevation angle).
        let mut t = wall(-1.0, -100.0, 100.0, 6.0);
        t.extend(wall(1.0, -100.0, 100.0, 6.0));
        let scene = SkyScene {
            triangles: t,
            crowns: Vec::new(),
            ground: Ground::flat(0.0),
        };
        let analytic = (1.0f32 / 6.0).atan().sin();
        let traced = sky_view_at(&scene, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0], 8192);
        assert!(
            (traced / analytic - 1.0).abs() < 0.05,
            "{traced} vs {analytic}"
        );
        // Beside a single such wall, 3 m away: (1 + cos(atan(6 / 3))) / 2.
        let one = SkyScene {
            triangles: wall(-1.0, -100.0, 100.0, 6.0),
            ..scene.clone()
        };
        let beside = sky_view_at(&one, [2.0, 0.0, 0.0], [0.0, 1.0, 0.0], 8192);
        let expected = 0.5 * (1.0 + (6.0f32 / 3.0).atan().cos());
        assert!((beside - expected).abs() < 0.02, "{beside} {expected}");
    }

    fn tree() -> SkyScene {
        SkyScene {
            triangles: Vec::new(),
            crowns: vec![Canopy {
                center: [0.0, 5.0, 0.0],
                radii: [4.0, 3.5, 4.0],
                extinction: crown_extinction("flora:fagus-sylvatica", 3.5, 1.0),
            }],
            ground: Ground::flat(0.0),
        }
    }

    #[test]
    fn the_ground_under_a_dense_crown_sees_less_sky_than_beside_it() {
        let scene = tree();
        let under = sky_view_at(&scene, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0], 4096);
        let beside = sky_view_at(&scene, [9.0, 0.0, 0.0], [0.0, 1.0, 0.0], 4096);
        assert!(under < 0.6 * beside, "{under} {beside}");
        assert!(under > 0.1, "a crown is no roof: {under}");
        // A leafless crown lets more through.
        let mut bare = scene.clone();
        bare.crowns[0].extinction = crown_extinction("flora:fagus-sylvatica", 3.5, 0.0);
        let winter = sky_view_at(&bare, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0], 4096);
        assert!(winter > under + 0.2, "{winter} {under}");
        // The baked grid agrees.
        let v = bake(scene).expect("a crown makes a grid");
        let (gu, gb) = (
            v.sky_view([0.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            v.sky_view([6.5, 0.0, 0.0], [0.0, 1.0, 0.0]),
        );
        assert!(gu < 0.7 * gb && gb > 0.75, "{gu} {gb}");
        assert!((gu - under).abs() < 0.12, "{gu} {under}");
    }

    #[test]
    fn a_probe_inside_a_closed_box_is_left_out() {
        // A closed 4 x 3 x 4 m box.
        let (a, b) = ([0.0, 0.0, 0.0], [4.0, 3.0, 4.0]);
        let c = |x: usize, y: usize, z: usize| {
            [
                if x == 0 { a[0] } else { b[0] },
                if y == 0 { a[1] } else { b[1] },
                if z == 0 { a[2] } else { b[2] },
            ]
        };
        let mut t = Vec::new();
        t.extend(quad(c(0, 0, 0), c(1, 0, 0), c(1, 0, 1), c(0, 0, 1)));
        t.extend(quad(c(0, 1, 0), c(1, 1, 0), c(1, 1, 1), c(0, 1, 1)));
        t.extend(quad(c(0, 0, 0), c(0, 1, 0), c(0, 1, 1), c(0, 0, 1)));
        t.extend(quad(c(1, 0, 0), c(1, 1, 0), c(1, 1, 1), c(1, 0, 1)));
        t.extend(quad(c(0, 0, 0), c(1, 0, 0), c(1, 1, 0), c(0, 1, 0)));
        t.extend(quad(c(0, 0, 1), c(1, 0, 1), c(1, 1, 1), c(0, 1, 1)));
        let v = bake(SkyScene {
            triangles: t,
            crowns: Vec::new(),
            ground: Ground::flat(0.0),
        })
        .unwrap();
        // Inside, nothing outdoors round the point; outside the wall, open beyond it.
        assert!(v.sample([2.0, 1.0, 2.0]).is_none());
        let by_wall = v.sky_view([-0.5, 0.0, 2.0], [0.0, 1.0, 0.0]);
        let far = v.sky_view([-2.9, 0.0, 2.0], [0.0, 1.0, 0.0]);
        assert!(by_wall < far && far > 0.8, "{by_wall} {far}");
        // The wall's foot facing it sees less sky than facing away.
        assert!(
            v.sky_view([-0.5, 1.0, 2.0], [1.0, 0.0, 0.0])
                < v.sky_view([-0.5, 1.0, 2.0], [-1.0, 0.0, 0.0])
        );
        // The texture: three tiles of the weighted coefficients.
        let tex = v.texels();
        assert_eq!(tex.len(), 3 * v.sh.len());
    }

    #[test]
    fn a_bake_is_deterministic_and_stepping_gives_the_same() {
        let mut scene = tree();
        scene.triangles = wall(4.0, -3.0, 3.0, 2.5);
        let a = bake(scene.clone()).unwrap();
        let b = bake(scene.clone()).unwrap();
        assert_eq!(a, b);
        let mut job = SkyBake::new(scene.clone()).unwrap();
        while !job.step(97) {}
        assert_eq!(job.finish(), a);
    }

    #[test]
    fn a_scene_and_its_bake_survive_the_trip_through_floats() {
        let mut scene = tree();
        scene.triangles = wall(4.0, -3.0, 3.0, 2.5);
        scene.ground = Ground::sample([-5.0, -5.0], [5.0, 5.0], 1.0, |x, z| 0.1 * x + 0.05 * z);
        assert_eq!(SkyScene::from_floats(&scene.to_floats()), Some(scene.clone()));
        let v = bake(scene).unwrap();
        assert_eq!(SkyVisibility::from_floats(&v.to_floats()), Some(v));
        assert_eq!(SkyScene::from_floats(&[1.0]), None);
    }

    #[test]
    fn a_change_of_the_design_changes_the_key_and_the_bake() {
        let scene = tree();
        let mut grown = scene.clone();
        grown.crowns[0].radii = [4.5, 4.0, 4.5];
        assert_ne!(scene.key(), grown.key());
        assert_eq!(scene.key(), tree().key());
        let (a, b) = (bake(scene).unwrap(), bake(grown).unwrap());
        assert_ne!(a, b);
        // A grown crown shades more of the ground beside its old edge.
        let p = [4.2, 0.0, 0.0];
        assert!(b.sky_view(p, [0.0, 1.0, 0.0]) < a.sky_view(p, [0.0, 1.0, 0.0]));
        // Hilly ground, as a terrain grid, blocks the low sky: a valley floor sees less.
        let valley = Ground::sample([-30.0, -30.0], [30.0, 30.0], 1.0, |x, _| {
            (x.abs() - 4.0).max(0.0) * 0.8
        });
        let s = SkyScene {
            triangles: Vec::new(),
            crowns: Vec::new(),
            ground: valley,
        };
        let floor = sky_view_at(&s, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0], 4096);
        assert!(floor < 0.85 && floor > 0.4, "{floor}");
    }
}
