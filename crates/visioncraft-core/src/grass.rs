//! The grass mask: where lawns and meadows grow, as a grid of cells seen from above, for the
//! renderer's GPU grass. The renderer generates the blades every frame round the eye from this
//! grid (a compute pass, as in "Procedural Grass in Ghost of Tsushima", E. Wohllaib, GDC 2021),
//! so it is made once per design change, never per frame.
//!
//! Each cell holds the grass's density (relative to [`FULL_DENSITY_PER_M2`]), its blade height
//! (square-root coded up to [`MAX_BLADE_HEIGHT_M`]), its flowers and the index of its lawn
//! material (the renderer's colour palette), plus the height of the ground under it.

use crate::ImportedModel;
use crate::material::{Lawn3d, MAX_BLADE_HEIGHT_M};
use crate::planting::{LAWN_MAX_SLOPE_DEG, point_in_polygon};

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
    let tris: Vec<_> = patches.iter().map(patch_triangles).collect();
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    for (t, _) in tris.iter().flatten() {
        for p in t {
            lo = [lo[0].min(p[0]), lo[1].min(p[2])];
            hi = [hi[0].max(p[0]), hi[1].max(p[2])];
        }
    }
    if !lo[0].is_finite() || !hi[0].is_finite() {
        return None;
    }
    let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    let cell = MIN_CELL_M.max(extent / (MAX_SIDE - 2 * HEIGHT_MARGIN as u32 - 2) as f32);
    let margin = (HEIGHT_MARGIN + 1) as f32 * cell;
    let origin = [lo[0] - margin, lo[1] - margin];
    let size = [
        (((hi[0] - origin[0] + margin) / cell).ceil() as u32).clamp(1, MAX_SIDE),
        (((hi[1] - origin[1] + margin) / cell).ceil() as u32).clamp(1, MAX_SIDE),
    ];
    let (w, h) = (size[0] as usize, size[1] as usize);
    let mut params = vec![[0u8; 4]; w * h];
    let mut heights = vec![0.0f32; w * h];
    let mut set = vec![false; w * h];
    let texel = |x: f32| ((x / cell) - 0.5).ceil().max(0.0) as usize;
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
                let z = origin[1] + (iz as f32 + 0.5) * cell;
                for ix in x0..x1 {
                    let x = origin[0] + (ix as f32 + 0.5) * cell;
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
                    if crowns
                        .iter()
                        .any(|c| (x - c[0]).powi(2) + (z - c[1]).powi(2) < c[2] * c[2])
                    {
                        keep *= UNDER_CROWN;
                    }
                    let i = iz * w + ix;
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

    fn at(m: &GrassMask, x: f32, z: f32) -> ([u8; 4], f32) {
        let ix = ((x - m.origin[0]) / m.cell_m) as usize;
        let iz = ((z - m.origin[1]) / m.cell_m) as usize;
        let i = iz * m.size[0] as usize + ix;
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
}
