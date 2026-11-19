//! The grass mask: where lawns and meadows grow, as a grid of cells seen from above, for the
//! renderer's GPU grass. The renderer generates the blades every frame round the eye from this
//! grid (a compute pass, as in "Procedural Grass in Ghost of Tsushima", E. Wohllaib, GDC 2021),
//! so it is made once per design change, never per frame.
//!
//! Each cell holds the grass's density (relative to [`FULL_DENSITY_PER_M2`]), its blade height
//! (square-root coded up to [`MAX_BLADE_HEIGHT_M`]), its flowers and the index of its lawn
//! material (the renderer's colour palette), plus the height of the ground under it.
//!
//! Besides the lawn faces of shapes and models, a terrain grows grass where its drawn layer
//! weights (painted, and the automatic materials' share of the base) are a lawn or a meadow
//! ([`TerrainGrass`]): the blades thin out where rock, scree or soil take over, so they follow the
//! ground's own blend. A terrain larger than [`WINDOW_M`] gets a mask round the camera
//! ([`window_round`]), made again when the camera moves to another [`WINDOW_STEP_M`] cell.

use crate::ImportedModel;
use crate::material::{Lawn3d, MAX_BLADE_HEIGHT_M};
use crate::planting::{LAWN_MAX_SLOPE_DEG, Rng, mix, point_in_polygon};
use crate::terrain::Terrain;

/// The longest side of the mask (cells).
pub const MAX_SIDE: u32 = 2048;
/// The finest cell (m): finer than the blades' clumps, coarse enough that a 100 m estate's lawns
/// fit in a few megabytes.
pub const MIN_CELL_M: f32 = 0.08;
/// A cell's density 1 stands for this many shoots per square metre (a dense lawn).
pub const FULL_DENSITY_PER_M2: f32 = 2000.0;
/// Under a tree's crown this share of the grass is left (shade thins it).
const UNDER_CROWN: f32 = 0.4;
/// Cells round a lawn that take the height of the lawn next to them (density 0), so that a
/// bilinear lookup at the edge never pulls the blades' roots down to a height of 0.
const HEIGHT_MARGIN: usize = 2;
/// The longest side of a mask with terrain grass: a terrain's grass is a meadow or a lawn seen
/// over tens of metres, an eighth of a metre per cell is fine enough, and a mask round the camera
/// stays a few megabytes to make again.
pub const TERRAIN_SIDE: u32 = 1024;
/// A terrain wider or deeper than this (m) grows its grass in a square of this side round the
/// camera: twice the farthest reach of the blades (`visioncraft-render` grass, ULTRA about 55 m)
/// with room for the camera to move within its cell.
pub const WINDOW_M: f32 = 128.0;
/// The camera's cells (m): the window is made again when the camera moves to another.
pub const WINDOW_STEP_M: f32 = 16.0;
/// Terrain grass thins from this share of grass layers (0..1) to full at `GRASS_FULL`: the
/// terrain shader's height blend shows rock or scree through below about a half.
const GRASS_FROM: f32 = 0.3;
const GRASS_FULL: f32 = 0.75;
/// Terrain grass grows on slopes up to here (degrees), thinning from `TERRAIN_SLOPE_FADE_DEG`
/// below it: steeper turf slides (and the automatic materials show scree there anyway).
const TERRAIN_MAX_SLOPE_DEG: f32 = 48.0;
const TERRAIN_SLOPE_FADE_DEG: f32 = 10.0;
/// The terrain grass's edge frays by noise of this size (m) moving the share by up to this much.
const EDGE_NOISE_M: f32 = 1.2;
const EDGE_FRAY: f32 = 0.22;
const EDGE_SEED: u64 = 0x6EA55;

/// One lawn face to grow grass on.
pub struct GrassPatch<'a> {
    pub model: &'a ImportedModel,
    /// An index into `model.faces`.
    pub face: usize,
    /// The model's matrix (columns), model to world.
    pub xf: [f32; 16],
    pub lawn: Lawn3d,
    /// The palette entry (renderer) of this lawn's material.
    pub kind: u8,
    /// Outlines without grass (world x, z): terraces, paths and ponds over the lawn.
    pub holes: &'a [Vec<[f32; 2]>],
    /// The soft edge width of each of `holes` (m; missing or 0: hard): the grass carries on into
    /// the band, thinning to none at its inner side.
    pub hole_blends: &'a [f32],
}

/// A terrain's grass: its layers that are lawns or meadows.
pub struct TerrainGrass<'a> {
    pub terrain: &'a Terrain,
    /// Per layer of `terrain`: its lawn (stripes off: those belong to a drawn lawn) and palette
    /// entry when the layer's material is a lawn or a meadow.
    pub layers: Vec<Option<(Lawn3d, u8)>>,
    /// Outlines over the terrain without grass (world x, z): areas drawn on it, terraces' holes,
    /// models lying on the ground.
    pub holes: &'a [Vec<[f32; 2]>],
    /// Ground below this height grows none (a sea's or a lake's surface).
    pub min_height: Option<f32>,
}

impl TerrainGrass<'_> {
    /// Whether any layer is grass.
    pub fn any(&self) -> bool {
        self.layers.iter().any(Option::is_some)
    }

    /// Whether the terrain is too big for one mask: its grass then grows round the camera.
    pub fn windowed(&self) -> bool {
        let [lo, hi] = self.terrain.extent();
        hi[0] - lo[0] > WINDOW_M || hi[1] - lo[1] > WINDOW_M
    }
}

/// The camera's cell (`WINDOW_STEP_M` squares) for `eye` (world x, z).
pub fn window_cell(eye: [f32; 2]) -> [i32; 2] {
    eye.map(|v| (v / WINDOW_STEP_M).floor() as i32)
}

/// The window round the camera's cell `cell`: [`WINDOW_M`] square, centred on the cell.
pub fn window_round(cell: [i32; 2]) -> [[f32; 2]; 2] {
    let c = cell.map(|k| (k as f32 + 0.5) * WINDOW_STEP_M);
    let h = WINDOW_M * 0.5;
    [[c[0] - h, c[1] - h], [c[0] + h, c[1] + h]]
}

fn sstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Per terrain vertex in a range: the share of grass (thinned on steep ground) and the
/// strongest grass layer there.
struct TerrainShares {
    /// First vertex (ix, iz) and the count along x and z.
    first: [u32; 2],
    count: [u32; 2],
    share: Vec<f32>,
    layer: Vec<u8>,
}

impl TerrainShares {
    fn new(g: &TerrainGrass, lo: [f32; 2], hi: [f32; 2]) -> Option<Self> {
        let t = g.terrain;
        let vertex = |x: f32, o: f32, n: u32, up: bool| {
            let u = (x - o) / t.cell_m;
            let u = if up { u.ceil() } else { u.floor() };
            u.clamp(0.0, n as f32) as u32
        };
        let first = [
            vertex(lo[0], t.origin[0], t.width, false),
            vertex(lo[1], t.origin[1], t.depth, false),
        ];
        let last = [
            vertex(hi[0], t.origin[0], t.width, true),
            vertex(hi[1], t.origin[1], t.depth, true),
        ];
        if last[0] <= first[0] || last[1] <= first[1] {
            return None;
        }
        let count = [last[0] - first[0] + 1, last[1] - first[1] + 1];
        let n = count[0] as usize * count[1] as usize;
        let (mut share, mut layer) = (vec![0.0f32; n], vec![u8::MAX; n]);
        let cos_max = TERRAIN_MAX_SLOPE_DEG.to_radians().cos();
        let cos_fade = (TERRAIN_MAX_SLOPE_DEG - TERRAIN_SLOPE_FADE_DEG)
            .to_radians()
            .cos();
        let mut any = false;
        for iz in 0..count[1] {
            for ix in 0..count[0] {
                let (gx, gz) = (first[0] + ix, first[1] + iz);
                let w = t.vertex_weights(gx, gz);
                let mut sum = 0u32;
                let mut best = (0u8, u8::MAX);
                for (l, lawn) in g.layers.iter().enumerate().take(w.len()) {
                    if lawn.is_some() && w[l] > 0 {
                        sum += u32::from(w[l]);
                        if w[l] > best.0 {
                            best = (w[l], l as u8);
                        }
                    }
                }
                if sum == 0 {
                    continue;
                }
                let [x, z] = t.xz(gx, gz);
                let up = t.normal_at(x, z)[1];
                let s = (sum as f32 / 255.0) * sstep(cos_max, cos_fade, up);
                let i = iz as usize * count[0] as usize + ix as usize;
                share[i] = s;
                layer[i] = best.1;
                any |= s > 0.0;
            }
        }
        any.then_some(Self {
            first,
            count,
            share,
            layer,
        })
    }

    /// The world bounds of the vertices with any grass: (min, max).
    fn bounds(&self, t: &Terrain) -> Option<([f32; 2], [f32; 2])> {
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for iz in 0..self.count[1] {
            for ix in 0..self.count[0] {
                if self.share[(iz * self.count[0] + ix) as usize] > 0.0 {
                    let [x, z] = t.xz(self.first[0] + ix, self.first[1] + iz);
                    lo = [lo[0].min(x), lo[1].min(z)];
                    hi = [hi[0].max(x), hi[1].max(z)];
                }
            }
        }
        lo[0].is_finite().then_some((lo, hi))
    }
}

/// Where each cell centre of a mask's row (or column) falls on a lattice of `step` metres whose
/// first point is at `o`, with `n` points: the lattice cell (its first point) and the fraction
/// across it; `None` outside the lattice.
fn lattice_axis(
    origin: f32,
    cell: f32,
    count: usize,
    o: f32,
    step: f32,
    n: usize,
) -> Vec<Option<(usize, f32)>> {
    (0..count)
        .map(|i| {
            let u = (origin + (i as f32 + 0.5) * cell - o) / step;
            if n < 2 || u < 0.0 || u > (n - 1) as f32 {
                return None;
            }
            let k = (u.floor() as usize).min(n - 2);
            Some((k, u - k as f32))
        })
        .collect()
}

/// [`value_noise`] over a rectangle, its lattice hashed once: the same values, for the many
/// cells of a mask.
struct LatticeNoise {
    scale: f32,
    first: [i64; 2],
    count: [usize; 2],
    values: Vec<f32>,
}

impl LatticeNoise {
    /// Noise of lattice size `scale` (m) covering world `lo..hi`.
    fn new(seed: u64, scale: f32, lo: [f32; 2], hi: [f32; 2]) -> Self {
        let first = lo.map(|v| (v / scale).floor() as i64);
        let last = hi.map(|v| (v / scale).floor() as i64 + 1);
        let count = [0, 1].map(|k| (last[k] - first[k] + 1).max(2) as usize);
        let mut values = Vec::with_capacity(count[0] * count[1]);
        for iz in 0..count[1] as i64 {
            for ix in 0..count[0] as i64 {
                let (gx, gz) = (first[0] + ix, first[1] + iz);
                values.push(Rng::new(mix(mix(seed, gx), gz)).f32() * 2.0 - 1.0);
            }
        }
        Self {
            scale,
            first,
            count,
            values,
        }
    }

    /// The lattice cells of a mask's columns (`along` 0) or rows (1), the fractions smoothed.
    fn axis(&self, along: usize, origin: f32, cell: f32, count: usize) -> Vec<(usize, f32)> {
        let o = self.first[along] as f32 * self.scale;
        lattice_axis(origin, cell, count, o, self.scale, self.count[along])
            .into_iter()
            .map(|a| {
                let (k, f) = a.unwrap_or((0, 0.0));
                (k, f * f * (3.0 - 2.0 * f))
            })
            .collect()
    }

    /// The noise at a column and a row from [`LatticeNoise::axis`], in `-1..1`.
    fn at(&self, col: (usize, f32), row: (usize, f32)) -> f32 {
        let w = self.count[0];
        let i = row.0 * w + col.0;
        let v = &self.values;
        let a = v[i] + (v[i + 1] - v[i]) * col.1;
        let b = v[i + w] + (v[i + w + 1] - v[i + w]) * col.1;
        a + (b - a) * row.1
    }
}

/// The grid: row-major from (`origin`), x along a row, z down the rows.
#[derive(Clone, Debug, PartialEq)]
pub struct GrassMask {
    /// World x, z of the first cell's corner.
    pub origin: [f32; 2],
    pub cell_m: f32,
    /// Cells along x and z.
    pub size: [u32; 2],
    /// Per cell: density (0..255 of [`FULL_DENSITY_PER_M2`]), blade height ([`encode_height`]),
    /// flowers (0..255) and the palette entry.
    pub params: Vec<[u8; 4]>,
    /// Per cell: the ground's world height (m).
    pub heights: Vec<f32>,
}

/// A blade height as stored: the square root of its share of [`MAX_BLADE_HEIGHT_M`], so a mown
/// lawn's few centimetres keep millimetre steps.
pub fn encode_height(h: f32) -> u8 {
    ((h / MAX_BLADE_HEIGHT_M).clamp(0.0, 1.0).sqrt() * 255.0).round() as u8
}

/// The inverse of [`encode_height`] (the renderer's shader decodes the same way).
pub fn decode_height(v: u8) -> f32 {
    let s = f32::from(v) / 255.0;
    s * s * MAX_BLADE_HEIGHT_M
}

fn transform(m: &[f32; 16], p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
}

/// The triangles of a patch flat enough for blades (world), each corner with its soft edge
/// value (1: no edge).
fn patch_triangles(p: &GrassPatch) -> Vec<([[f32; 3]; 3], [f32; 3])> {
    let min_up = LAWN_MAX_SLOPE_DEG.to_radians().cos();
    let m = p.model;
    m.triangle_faces
        .iter()
        .enumerate()
        .filter(|(_, f)| **f as usize == p.face)
        .filter_map(|(t, _)| {
            let idx = m.triangles[t];
            let [a, b, c] = idx.map(|i| transform(&p.xf, m.positions[i as usize]));
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len == 0.0 || n[1].abs() / len < min_up {
                return None;
            }
            let edge = idx.map(|i| m.edge.get(i as usize).copied().unwrap_or(1.0));
            Some(([a, b, c], edge))
        })
        .collect()
}

/// A hole's bounds (min x, min z, max x, max z), outline and soft edge width.
type Hole<'a> = ([f32; 4], &'a [[f32; 2]], f32);

/// Distance from `p` to the outline `poly` (m).
fn outline_distance(p: [f32; 2], poly: &[[f32; 2]]) -> f32 {
    let mut best = f32::INFINITY;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let ab = [b[0] - a[0], b[1] - a[1]];
        let ap = [p[0] - a[0], p[1] - a[1]];
        let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / (ab[0] * ab[0] + ab[1] * ab[1]).max(1e-12))
            .clamp(0.0, 1.0);
        let d = [ap[0] - ab[0] * t, ap[1] - ab[1] * t];
        best = best.min((d[0] * d[0] + d[1] * d[1]).sqrt());
    }
    best
}

/// The mask over every patch, or `None` when no patch has a triangle flat enough for grass.
/// Later patches overwrite earlier ones where they overlap. `crowns` are tree crowns' circles
/// (x, z, radius), where the grass thins.
pub fn grass_mask(patches: &[GrassPatch], crowns: &[[f32; 3]]) -> Option<GrassMask> {
    grass_mask_in(patches, None, crowns, None)
}

/// [`grass_mask`] with a terrain's grass under the patches (which overwrite it), only within
/// `window` (min, max world x, z) when given: a terrain too big for one mask
/// ([`TerrainGrass::windowed`]) passes [`window_round`] of the camera's cell.
pub fn grass_mask_in(
    patches: &[GrassPatch],
    terrain: Option<&TerrainGrass>,
    crowns: &[[f32; 3]],
    window: Option<[[f32; 2]; 2]>,
) -> Option<GrassMask> {
    let tris: Vec<_> = patches.iter().map(patch_triangles).collect();
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    for (t, _) in tris.iter().flatten() {
        for p in t {
            lo = [lo[0].min(p[0]), lo[1].min(p[2])];
            hi = [hi[0].max(p[0]), hi[1].max(p[2])];
        }
    }
    let shares = terrain.filter(|g| g.any()).and_then(|g| {
        let [tlo, thi] = g.terrain.extent();
        let (wlo, whi) = window.map_or((tlo, thi), |[a, b]| {
            (
                [a[0].max(tlo[0]), a[1].max(tlo[1])],
                [b[0].min(thi[0]), b[1].min(thi[1])],
            )
        });
        TerrainShares::new(g, wlo, whi).map(|s| (g, s))
    });
    let mut side = MAX_SIDE;
    if let Some((g, s)) = &shares
        && let Some((a, b)) = s.bounds(g.terrain)
    {
        lo = [lo[0].min(a[0]), lo[1].min(a[1])];
        hi = [hi[0].max(b[0]), hi[1].max(b[1])];
        side = TERRAIN_SIDE;
    }
    if let Some([a, b]) = window {
        lo = [lo[0].max(a[0]), lo[1].max(a[1])];
        hi = [hi[0].min(b[0]), hi[1].min(b[1])];
    }
    if !lo[0].is_finite() || !hi[0].is_finite() || hi[0] <= lo[0] || hi[1] <= lo[1] {
        return None;
    }
    let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    let cell = MIN_CELL_M.max(extent / (side - 2 * HEIGHT_MARGIN as u32 - 2) as f32);
    let margin = (HEIGHT_MARGIN + 1) as f32 * cell;
    let origin = [lo[0] - margin, lo[1] - margin];
    let size = [
        (((hi[0] - origin[0] + margin) / cell).ceil() as u32).clamp(1, side),
        (((hi[1] - origin[1] + margin) / cell).ceil() as u32).clamp(1, side),
    ];
    let (w, h) = (size[0] as usize, size[1] as usize);
    let mut params = vec![[0u8; 4]; w * h];
    let mut heights = vec![0.0f32; w * h];
    let mut set = vec![false; w * h];
    let texel = |x: f32| ((x / cell) - 0.5).ceil().max(0.0) as usize;
    let centre = |i: usize, o: f32| o + (i as f32 + 0.5) * cell;
    // The cells under a crown, by each crown's square of cells.
    let mut shaded = vec![false; w * h];
    for c in crowns {
        let (x0, x1) = (
            texel(c[0] - c[2] - origin[0]),
            texel(c[0] + c[2] - origin[0]).min(w),
        );
        let (z0, z1) = (
            texel(c[1] - c[2] - origin[1]),
            texel(c[1] + c[2] - origin[1]).min(h),
        );
        for iz in z0..z1 {
            let z = centre(iz, origin[1]);
            for ix in x0..x1 {
                let x = centre(ix, origin[0]);
                if (x - c[0]).powi(2) + (z - c[1]).powi(2) < c[2] * c[2] {
                    shaded[iz * w + ix] = true;
                }
            }
        }
    }
    if let Some((g, s)) = &shares {
        let t = g.terrain;
        // The cells under what is drawn over the terrain.
        let mut bare = vec![false; w * h];
        for hole in g.holes.iter().filter(|h| h.len() >= 3) {
            let b = hole
                .iter()
                .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, p| {
                    [
                        b[0].min(p[0]),
                        b[1].min(p[1]),
                        b[2].max(p[0]),
                        b[3].max(p[1]),
                    ]
                });
            let (x0, x1) = (texel(b[0] - origin[0]), texel(b[2] - origin[0]).min(w));
            let (z0, z1) = (texel(b[1] - origin[1]), texel(b[3] - origin[1]).min(h));
            for iz in z0..z1 {
                let z = centre(iz, origin[1]);
                for ix in x0..x1 {
                    if point_in_polygon([centre(ix, origin[0]), z], hole) {
                        bare[iz * w + ix] = true;
                    }
                }
            }
        }
        let far = [origin[0] + w as f32 * cell, origin[1] + h as f32 * cell];
        let coarse = LatticeNoise::new(EDGE_SEED, EDGE_NOISE_M, origin, far);
        let fine = LatticeNoise::new(EDGE_SEED ^ 1, EDGE_NOISE_M * 0.3, origin, far);
        // Each column's and row's place on the lattices (the terrain's vertices in the shares'
        // range and the two noises), made once: the loop over the cells only interpolates.
        let vertex = |k: usize| {
            lattice_axis(
                origin[k],
                cell,
                [w, h][k],
                t.origin[k] + s.first[k] as f32 * t.cell_m,
                t.cell_m,
                s.count[k] as usize,
            )
        };
        let (cols, rows) = (vertex(0), vertex(1));
        let noise_cols = [
            coarse.axis(0, origin[0], cell, w),
            fine.axis(0, origin[0], cell, w),
        ];
        let noise_rows = [
            coarse.axis(1, origin[1], cell, h),
            fine.axis(1, origin[1], cell, h),
        ];
        let sw = s.count[0] as usize;
        let tw = t.width as usize + 1;
        for iz in 0..h {
            let Some((vz, fz)) = rows[iz] else {
                continue;
            };
            for ix in 0..w {
                let i = iz * w + ix;
                let Some((vx, fx)) = cols[ix] else {
                    continue;
                };
                if bare[i] {
                    continue;
                }
                let j = vz * sw + vx;
                let sh = &s.share;
                let share = (sh[j] + (sh[j + 1] - sh[j]) * fx) * (1.0 - fz)
                    + (sh[j + sw] + (sh[j + sw + 1] - sh[j + sw]) * fx) * fz;
                // Where the grass gives out its edge frays into tufts and bare gaps (two octaves
                // of noise, a hand and a stride across), not a line along the weights' contour.
                // The noise is only made in that band.
                let keep = if share + EDGE_FRAY <= GRASS_FROM {
                    continue;
                } else if share - EDGE_FRAY >= GRASS_FULL {
                    1.0
                } else {
                    let fray = coarse.at(noise_cols[0][ix], noise_rows[0][iz]) * 0.65
                        + fine.at(noise_cols[1][ix], noise_rows[1][iz]) * 0.35;
                    sstep(GRASS_FROM, GRASS_FULL, share + fray * EDGE_FRAY)
                };
                if keep <= 0.0 {
                    continue;
                }
                let near = j + usize::from(fz >= 0.5) * sw + usize::from(fx >= 0.5);
                let Some((lawn, kind)) =
                    g.layers.get(usize::from(s.layer[near])).copied().flatten()
                else {
                    continue;
                };
                // The height on the triangles the terrain is drawn with (`surface_height`).
                let k = (vz + s.first[1] as usize) * tw + vx + s.first[0] as usize;
                let hh = &t.heights;
                let y = if fz >= fx {
                    hh[k] + (hh[k + tw] - hh[k]) * fz + (hh[k + tw + 1] - hh[k + tw]) * fx
                } else {
                    hh[k] + (hh[k + 1] - hh[k]) * fx + (hh[k + tw + 1] - hh[k + 1]) * fz
                };
                if g.min_height.is_some_and(|m| y < m) {
                    continue;
                }
                let thin = if shaded[i] { keep * UNDER_CROWN } else { keep };
                let density = (lawn.density_per_m2 / FULL_DENSITY_PER_M2).clamp(0.02, 1.0);
                // Where the grass gives out it is shorter too, not a cut edge of full height.
                let blade = lawn.blade_height_m * (0.55 + 0.45 * keep);
                params[i] = [
                    (density * thin * 255.0).round() as u8,
                    encode_height(blade),
                    (lawn.flowers.clamp(0.0, 1.0) * keep * 255.0).round() as u8,
                    kind,
                ];
                heights[i] = y;
                set[i] = true;
            }
        }
    }
    for (patch, tris) in patches.iter().zip(&tris) {
        let l = &patch.lawn;
        let density = (l.density_per_m2 / FULL_DENSITY_PER_M2).clamp(0.02, 1.0);
        let blade = encode_height(l.blade_height_m);
        let flowers = (l.flowers.clamp(0.0, 1.0) * 255.0).round() as u8;
        let holes: Vec<Hole> = patch
            .holes
            .iter()
            .enumerate()
            .filter(|(_, h)| h.len() >= 3)
            .map(|(i, h)| {
                let b = h
                    .iter()
                    .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, p| {
                        [
                            b[0].min(p[0]),
                            b[1].min(p[1]),
                            b[2].max(p[0]),
                            b[3].max(p[1]),
                        ]
                    });
                (
                    b,
                    h.as_slice(),
                    patch.hole_blends.get(i).copied().unwrap_or(0.0),
                )
            })
            .collect();
        for (t, edge) in tris {
            let [a, b, c] = *t;
            let det = (b[0] - a[0]) * (c[2] - a[2]) - (c[0] - a[0]) * (b[2] - a[2]);
            if det.abs() < 1e-12 {
                continue;
            }
            let x0 = texel(a[0].min(b[0]).min(c[0]) - origin[0]);
            let x1 = texel(a[0].max(b[0]).max(c[0]) - origin[0]).min(w);
            let z0 = texel(a[2].min(b[2]).min(c[2]) - origin[1]);
            let z1 = texel(a[2].max(b[2]).max(c[2]) - origin[1]).min(h);
            for iz in z0..z1 {
                let z = centre(iz, origin[1]);
                for ix in x0..x1 {
                    let x = centre(ix, origin[0]);
                    // Barycentric weights in x, z, a hair of tolerance so that two triangles
                    // sharing an edge leave no gap.
                    let wb = ((x - a[0]) * (c[2] - a[2]) - (c[0] - a[0]) * (z - a[2])) / det;
                    let wc = ((b[0] - a[0]) * (z - a[2]) - (x - a[0]) * (b[2] - a[2])) / det;
                    let wa = 1.0 - wb - wc;
                    if wa < -1e-4 || wb < -1e-4 || wc < -1e-4 {
                        continue;
                    }
                    let mut keep = wa * edge[0] + wb * edge[1] + wc * edge[2];
                    for (bb, poly, blend) in &holes {
                        if x < bb[0] || x > bb[2] || z < bb[1] || z > bb[3] {
                            continue;
                        }
                        if point_in_polygon([x, z], poly) {
                            keep *= if *blend > 0.0 {
                                1.0 - (outline_distance([x, z], poly) / blend).clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
                        }
                    }
                    let i = iz * w + ix;
                    if shaded[i] {
                        keep *= UNDER_CROWN;
                    }
                    params[i] = [
                        (density * keep.clamp(0.0, 1.0) * 255.0).round() as u8,
                        blade,
                        flowers,
                        patch.kind,
                    ];
                    heights[i] = wa * a[1] + wb * b[1] + wc * c[1];
                    set[i] = true;
                }
            }
        }
    }
    // The cells round the lawn take a neighbour's ground height (and blade height, for the
    // bilinear lookup), with no grass.
    for _ in 0..HEIGHT_MARGIN {
        let before = set.clone();
        for iz in 0..h {
            for ix in 0..w {
                let i = iz * w + ix;
                if before[i] {
                    continue;
                }
                let n = [
                    (ix > 0).then(|| i - 1),
                    (ix + 1 < w).then(|| i + 1),
                    (iz > 0).then(|| i - w),
                    (iz + 1 < h).then(|| i + w),
                ];
                if let Some(j) = n.into_iter().flatten().find(|&j| before[j]) {
                    heights[i] = heights[j];
                    params[i] = [0, params[j][1], 0, params[j][3]];
                    set[i] = true;
                }
            }
        }
    }
    Some(GrassMask {
        origin,
        cell_m: cell,
        size,
        params,
        heights,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Face;

    /// A flat square of `side` metres at height `y`, one face.
    fn square(side: f32, y: f32) -> ImportedModel {
        ImportedModel {
            positions: vec![
                [0.0, y, 0.0],
                [side, y, 0.0],
                [side, y, side],
                [0.0, y, side],
            ],
            normals: vec![[0.0, 1.0, 0.0]; 4],
            triangles: vec![[0, 2, 1], [0, 3, 2]],
            triangle_faces: vec![0, 0],
            faces: vec![Face {
                name: "top".into(),
                body: 0,
            }],
            bodies: Vec::new(),
            edge: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn lawn() -> Lawn3d {
        Lawn3d {
            density_per_m2: 1000.0,
            blade_height_m: 0.05,
            seed: 1,
            flowers: 0.5,
            ..Default::default()
        }
    }

    /// The cell at world (x, z); none (and no height) outside the mask.
    fn at(m: &GrassMask, x: f32, z: f32) -> ([u8; 4], f32) {
        let (u, v) = ((x - m.origin[0]) / m.cell_m, (z - m.origin[1]) / m.cell_m);
        if u < 0.0 || v < 0.0 || u >= m.size[0] as f32 || v >= m.size[1] as f32 {
            return ([0; 4], f32::NAN);
        }
        let i = v as usize * m.size[0] as usize + u as usize;
        (m.params[i], m.heights[i])
    }

    #[test]
    fn heights_round_trip_finely_for_a_lawn() {
        for h in [0.03, 0.04, 0.06, 0.45, 1.0] {
            let back = decode_height(encode_height(h));
            assert!((back - h).abs() < h * 0.06, "{h} -> {back}");
        }
    }

    #[test]
    fn a_lawn_is_covered_with_its_height_and_a_hole_is_bare() {
        let model = square(10.0, 0.3);
        // Lifted by 0.2 m: the matrix's translation.
        let mut xf = [0.0f32; 16];
        xf[0] = 1.0;
        xf[5] = 1.0;
        xf[10] = 1.0;
        xf[15] = 1.0;
        xf[13] = 0.2;
        let holes = vec![vec![[2.0, 2.0], [4.0, 2.0], [4.0, 4.0], [2.0, 4.0]]];
        let patch = GrassPatch {
            model: &model,
            face: 0,
            xf,
            lawn: lawn(),
            kind: 3,
            holes: &holes,
            hole_blends: &[],
        };
        let m = grass_mask(&[patch], &[[8.0, 8.0, 1.0]]).unwrap();
        assert!(m.cell_m >= MIN_CELL_M);
        let (p, y) = at(&m, 6.0, 6.0);
        assert_eq!(p[0], (0.5f32 * 255.0).round() as u8);
        assert_eq!(p[1], encode_height(0.05));
        assert_eq!(p[2], 128);
        assert_eq!(p[3], 3);
        assert!((y - 0.5).abs() < 1e-4, "{y}");
        // The hole: no grass, the ground's height kept.
        assert_eq!(at(&m, 3.0, 3.0).0[0], 0);
        assert!(at(&m, 3.0, 3.0).1 < 0.6);
        // Under the crown: thinner.
        let under = at(&m, 8.0, 8.0).0[0];
        assert!(under > 0 && under < p[0], "{under}");
        // Just off the lawn: no grass, but the lawn's height for the lookup.
        let (off, y) = at(&m, -0.1, 5.0);
        assert_eq!(off[0], 0);
        assert!((y - 0.5).abs() < 1e-4);
    }

    #[test]
    fn a_soft_hole_thins_the_grass_into_its_band() {
        let model = square(10.0, 0.0);
        let mut xf = [0.0f32; 16];
        (xf[0], xf[5], xf[10], xf[15]) = (1.0, 1.0, 1.0, 1.0);
        let holes = vec![vec![[2.0, 2.0], [8.0, 2.0], [8.0, 8.0], [2.0, 8.0]]];
        let patch = GrassPatch {
            model: &model,
            face: 0,
            xf,
            lawn: lawn(),
            kind: 0,
            holes: &holes,
            hole_blends: &[1.0],
        };
        let m = grass_mask(&[patch], &[]).unwrap();
        let outside = at(&m, 1.0, 5.0).0[0];
        let band = at(&m, 2.5, 5.0).0[0];
        let inside = at(&m, 5.0, 5.0).0[0];
        assert!(outside > band && band > inside, "{outside} {band} {inside}");
        assert_eq!(inside, 0);
    }

    #[test]
    fn a_large_estate_stays_within_the_side() {
        let model = square(400.0, 0.0);
        let mut xf = [0.0f32; 16];
        (xf[0], xf[5], xf[10], xf[15]) = (1.0, 1.0, 1.0, 1.0);
        let patch = GrassPatch {
            model: &model,
            face: 0,
            xf,
            lawn: lawn(),
            kind: 0,
            holes: &[],
            hole_blends: &[],
        };
        let m = grass_mask(&[patch], &[]).unwrap();
        assert!(m.size[0] <= MAX_SIDE && m.size[1] <= MAX_SIDE);
        assert!(m.cell_m > 0.19);
        assert_eq!(m.params.len(), (m.size[0] * m.size[1]) as usize);
    }

    /// A lawn terrain 40 by 40 m, flat for x < 20 and climbing at 50 degrees beyond, with the
    /// automatic materials: grass on the flat, rock on the slope.
    fn hillside() -> Terrain {
        let mut t = Terrain::flat([0.0, 0.0], 40.0, 40.0, 0.5, "lawn").unwrap();
        for iz in 0..=t.depth {
            for ix in 0..=t.width {
                let x = ix as f32 * t.cell_m;
                let i = t.at(ix, iz);
                t.heights[i] = if x > 20.0 { (x - 20.0) * 1.2 } else { 0.0 };
            }
        }
        for m in ["rock", "scree", "moss", "forest-floor"] {
            t.add_layer(m);
        }
        t.auto = Some(crate::natural::AutoMaterials {
            macro_strength: 0.0,
            ..Default::default()
        });
        t
    }

    fn lawn_layers(t: &Terrain) -> Vec<Option<(Lawn3d, u8)>> {
        t.layers
            .iter()
            .map(|l| (l.material == "lawn").then(|| (lawn(), 2)))
            .collect()
    }

    #[test]
    fn a_terrain_grows_grass_on_its_lawn_and_none_on_its_rock() {
        let t = hillside();
        let holes = vec![vec![[4.0, 4.0], [8.0, 4.0], [8.0, 8.0], [4.0, 8.0]]];
        let g = TerrainGrass {
            terrain: &t,
            layers: lawn_layers(&t),
            holes: &holes,
            min_height: None,
        };
        let m = grass_mask_in(&[], Some(&g), &[[12.0, 30.0, 2.0]], None).unwrap();
        assert!(m.size[0] <= TERRAIN_SIDE && m.size[1] <= TERRAIN_SIDE);
        let (p, y) = at(&m, 10.0, 20.0);
        assert_eq!(p[0], (0.5f32 * 255.0).round() as u8, "{p:?}");
        assert_eq!(p[3], 2);
        assert!(y.abs() < 1e-4, "{y}");
        // The rock face and what is drawn over the terrain: none.
        assert_eq!(at(&m, 30.0, 20.0).0[0], 0);
        assert_eq!(at(&m, 6.0, 6.0).0[0], 0);
        // Under a crown: thinner.
        let under = at(&m, 12.0, 30.0).0[0];
        assert!(under > 0 && under < p[0], "{under}");
        // Thinning and shortening towards the rock, not a cut edge.
        let edge: Vec<u8> = (0..60)
            .map(|k| at(&m, 16.0 + k as f32 * 0.1, 20.0).0[0])
            .collect();
        assert!(
            edge.iter().any(|d| *d > 0 && *d < p[0]),
            "no thinning: {edge:?}"
        );
    }

    #[test]
    fn a_lawn_area_over_the_terrain_wins_and_water_drowns_the_grass() {
        let t = hillside();
        let g = TerrainGrass {
            terrain: &t,
            layers: lawn_layers(&t),
            holes: &[],
            min_height: Some(0.5),
        };
        // Everything below 0.5 m is under water: only the slope's foot above it could grow
        // grass, and the rock there grows none.
        assert!(
            grass_mask_in(&[], Some(&g), &[], None)
                .is_none_or(|m| m.params.iter().all(|p| p[0] == 0))
        );
        let g = TerrainGrass {
            min_height: None,
            ..g
        };
        let model = square(5.0, 0.0);
        let mut xf = [0.0f32; 16];
        (xf[0], xf[5], xf[10], xf[15]) = (1.0, 1.0, 1.0, 1.0);
        xf[12] = 2.0;
        xf[14] = 2.0;
        let patch = GrassPatch {
            model: &model,
            face: 0,
            xf,
            lawn: Lawn3d {
                density_per_m2: 2000.0,
                ..lawn()
            },
            kind: 5,
            holes: &[],
            hole_blends: &[],
        };
        let m = grass_mask_in(&[patch], Some(&g), &[], None).unwrap();
        assert_eq!(at(&m, 4.0, 4.0).0[3], 5);
        assert_eq!(at(&m, 10.0, 10.0).0[3], 2);
    }

    #[test]
    fn the_cached_noise_is_the_terrain_noise() {
        let (origin, cell) = ([-3.0, 5.0], 0.13);
        let n = LatticeNoise::new(9, 0.36, origin, [4.0, 10.5]);
        let (cols, rows) = (
            n.axis(0, origin[0], cell, 50),
            n.axis(1, origin[1], cell, 40),
        );
        for (ix, iz) in [(0, 0), (7, 3), (49, 39), (20, 31), (33, 2)] {
            let (x, z) = (
                origin[0] + (ix as f32 + 0.5) * cell,
                origin[1] + (iz as f32 + 0.5) * cell,
            );
            let want = crate::terrain::value_noise(9, x / 0.36, z / 0.36);
            let got = n.at(cols[ix], rows[iz]);
            assert!((got - want).abs() < 1e-4, "{x} {z}: {got} {want}");
        }
    }

    #[test]
    fn a_large_terrain_grows_its_grass_round_the_camera() {
        let t = Terrain::flat([-300.0, -300.0], 600.0, 600.0, 3.0, "lawn").unwrap();
        let g = TerrainGrass {
            terrain: &t,
            layers: lawn_layers(&t),
            holes: &[],
            min_height: None,
        };
        assert!(g.windowed());
        let cell = window_cell([100.0, -40.0]);
        let w = window_round(cell);
        assert!(w[0][0] < 100.0 - WINDOW_M * 0.4 && w[1][0] > 100.0 + WINDOW_M * 0.4);
        let m = grass_mask_in(&[], Some(&g), &[], Some(w)).unwrap();
        assert!(m.size[0] <= TERRAIN_SIDE && m.size[1] <= TERRAIN_SIDE);
        assert!(m.cell_m <= 0.13, "{}", m.cell_m);
        assert!(m.origin[0] >= w[0][0] - 1.0 && m.origin[1] >= w[0][1] - 1.0);
        assert!(at(&m, 100.0, -40.0).0[0] > 0);
        // A small terrain is not windowed.
        let small = hillside();
        let g = TerrainGrass {
            terrain: &small,
            layers: lawn_layers(&small),
            holes: &[],
            min_height: None,
        };
        assert!(!g.windowed());
    }
}
