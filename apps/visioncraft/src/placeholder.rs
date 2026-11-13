//! Stand-in meshes for catalogue assets, sized by `size_m`. `parts` is the single function to
//! replace when real plant meshes exist.

use crate::scene::Look;
use std::f32::consts::{FRAC_PI_2, TAU};
use visioncraft_core::{Asset, catalog::Category};

#[derive(Default)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub face_ids: Vec<u32>,
}

impl MeshData {
    fn vertex(&mut self, p: [f32; 3], n: [f32; 3]) -> u32 {
        self.positions.push(p);
        self.normals.push(n);
        self.face_ids.push(0);
        self.positions.len() as u32 - 1
    }

    /// A ring of `sides` quads between two circles (radii, heights); open top and bottom.
    fn frustum(&mut self, (r0, y0): (f32, f32), (r1, y1): (f32, f32), sides: u32) {
        let slope = (r0 - r1) / (y1 - y0).max(1e-4);
        for i in 0..sides {
            let (a0, a1) = (
                i as f32 / sides as f32 * TAU,
                (i + 1) as f32 / sides as f32 * TAU,
            );
            let n = |a: f32| {
                let l = (1.0 + slope * slope).sqrt();
                [a.cos() / l, slope / l, a.sin() / l]
            };
            let v = [
                self.vertex([r0 * a0.cos(), y0, r0 * a0.sin()], n(a0)),
                self.vertex([r0 * a1.cos(), y0, r0 * a1.sin()], n(a1)),
                self.vertex([r1 * a1.cos(), y1, r1 * a1.sin()], n(a1)),
                self.vertex([r1 * a0.cos(), y1, r1 * a0.sin()], n(a0)),
            ];
            // Counter-clockwise seen from outside.
            self.indices.extend([v[0], v[3], v[2], v[0], v[2], v[1]]);
        }
    }

    /// Part of an ellipsoid from latitude `lat0` to `lat1` (radians, -pi/2..pi/2).
    fn ellipsoid(
        &mut self,
        c: [f32; 3],
        r: [f32; 3],
        (lat0, lat1): (f32, f32),
        segs: u32,
        rings: u32,
    ) {
        let base = self.positions.len() as u32;
        for j in 0..=rings {
            let lat = lat0 + (lat1 - lat0) * j as f32 / rings as f32;
            for i in 0..=segs {
                let a = i as f32 / segs as f32 * TAU;
                let d = [lat.cos() * a.cos(), lat.sin(), lat.cos() * a.sin()];
                let n = [d[0] / r[0], d[1] / r[1], d[2] / r[2]];
                let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                self.vertex(
                    [c[0] + r[0] * d[0], c[1] + r[1] * d[1], c[2] + r[2] * d[2]],
                    [n[0] / l, n[1] / l, n[2] / l],
                );
            }
        }
        for j in 0..rings {
            for i in 0..segs {
                let a = base + j * (segs + 1) + i;
                let b = a + segs + 1;
                self.indices.extend([a, b, a + 1, a + 1, b, b + 1]);
            }
        }
    }

    fn cone(&mut self, r: f32, y0: f32, y1: f32, sides: u32) {
        self.frustum((r, y0), (0.0, y1), sides);
    }

    fn cuboid(&mut self, min: [f32; 3], max: [f32; 3]) {
        let [a, b] = [min, max];
        // (normal, four corners counter-clockwise seen from outside)
        let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
            (
                [0.0, 1.0, 0.0],
                [
                    [a[0], b[1], b[2]],
                    [b[0], b[1], b[2]],
                    [b[0], b[1], a[2]],
                    [a[0], b[1], a[2]],
                ],
            ),
            (
                [0.0, -1.0, 0.0],
                [
                    [a[0], a[1], a[2]],
                    [b[0], a[1], a[2]],
                    [b[0], a[1], b[2]],
                    [a[0], a[1], b[2]],
                ],
            ),
            (
                [0.0, 0.0, 1.0],
                [
                    [a[0], a[1], b[2]],
                    [b[0], a[1], b[2]],
                    [b[0], b[1], b[2]],
                    [a[0], b[1], b[2]],
                ],
            ),
            (
                [0.0, 0.0, -1.0],
                [
                    [b[0], a[1], a[2]],
                    [a[0], a[1], a[2]],
                    [a[0], b[1], a[2]],
                    [b[0], b[1], a[2]],
                ],
            ),
            (
                [1.0, 0.0, 0.0],
                [
                    [b[0], a[1], b[2]],
                    [b[0], a[1], a[2]],
                    [b[0], b[1], a[2]],
                    [b[0], b[1], b[2]],
                ],
            ),
            (
                [-1.0, 0.0, 0.0],
                [
                    [a[0], a[1], a[2]],
                    [a[0], a[1], b[2]],
                    [a[0], b[1], b[2]],
                    [a[0], b[1], a[2]],
                ],
            ),
        ];
        for (n, c) in faces {
            let v = c.map(|p| self.vertex(p, n));
            self.indices.extend([v[0], v[1], v[2], v[0], v[2], v[3]]);
        }
    }

    /// A thin double-sided blade leaning by `lean` along +X.
    fn blade(&mut self, x: f32, z: f32, h: f32, w: f32, lean: f32) {
        let n = [0.0, 0.0, 1.0];
        let v = [
            self.vertex([x - w, 0.0, z], n),
            self.vertex([x + w, 0.0, z], n),
            self.vertex([x + lean, h, z], n),
        ];
        self.indices.extend([v[0], v[1], v[2], v[0], v[2], v[1]]);
    }
}

const BARK: Look = Look::plain(0.12, 0.07, 0.035, 0.9);
const LEAF: Look = Look::plain(0.05, 0.19, 0.04, 0.8);
const CONIFER: Look = Look::plain(0.02, 0.10, 0.05, 0.85);
const SHRUB: Look = Look::plain(0.06, 0.16, 0.05, 0.85);
const PERENNIAL: Look = Look::plain(0.20, 0.12, 0.28, 0.8);
const GRASS: Look = Look::plain(0.18, 0.25, 0.06, 0.8);
const OTHER: Look = Look::plain(0.5, 0.45, 0.4, 0.8);

/// Meshes with looks for one asset, standing on y = 0 and filling its `size_m` (width, height,
/// depth). An unknown asset is a one-metre box.
pub fn parts(asset: Option<&Asset>) -> Vec<(MeshData, Look)> {
    let Some(a) = asset else {
        let mut m = MeshData::default();
        m.cuboid([-0.5, 0.0, -0.5], [0.5, 1.0, 0.5]);
        return vec![(m, OTHER)];
    };
    let [w, h, d] = a.size_m;
    let (rx, rz) = (w / 2.0, d / 2.0);
    let conifer = a.tags.iter().any(|t| t == "conifer");
    let mut out = Vec::new();
    match a.category {
        Category::Tree if conifer => {
            let mut trunk = MeshData::default();
            trunk.frustum((w * 0.05, 0.0), (w * 0.03, h * 0.2), 8);
            let mut crown = MeshData::default();
            crown.cone(rx.max(rz), h * 0.12, h, 12);
            out.extend([(trunk, BARK), (crown, CONIFER)]);
        }
        Category::Tree => {
            let mut trunk = MeshData::default();
            trunk.frustum((w * 0.06, 0.0), (w * 0.035, h * 0.6), 8);
            let mut crown = MeshData::default();
            crown.ellipsoid(
                [0.0, h * 0.68, 0.0],
                [rx, h * 0.32, rz],
                (-FRAC_PI_2, FRAC_PI_2),
                14,
                8,
            );
            out.extend([(trunk, BARK), (crown, LEAF)]);
        }
        Category::Shrub | Category::Hedge | Category::Perennial | Category::Flower => {
            let mut mound = MeshData::default();
            mound.ellipsoid([0.0; 3], [rx, h, rz], (0.0, FRAC_PI_2), 12, 5);
            let look = if matches!(a.category, Category::Perennial | Category::Flower) {
                PERENNIAL
            } else {
                SHRUB
            };
            out.push((mound, look));
        }
        Category::Grass => {
            let mut tuft = MeshData::default();
            for i in 0..7 {
                let t = i as f32 / 7.0 * TAU;
                tuft.blade(
                    rx * 0.5 * t.cos(),
                    rz * 0.5 * t.sin(),
                    h * (0.7 + 0.3 * (i % 3) as f32 / 2.0),
                    w * 0.04,
                    rx * 0.4 * t.cos(),
                );
            }
            out.push((tuft, GRASS));
        }
        _ => {
            let mut b = MeshData::default();
            b.cuboid([-rx, 0.0, -rz], [rx, h, rz]);
            out.push((b, OTHER));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(category: Category, size: [f32; 3]) -> Asset {
        Asset {
            id: "a".into(),
            name: "a".into(),
            category,
            tags: vec![],
            license: "CC0-1.0".into(),
            origin: Default::default(),
            author: String::new(),
            source_url: String::new(),
            size_m: size,
            lods: vec![],
            thumbnail: String::new(),
            plant: None,
            light: None,
        }
    }

    #[test]
    fn every_category_fits_its_size_and_indexes_validly() {
        for c in [
            Category::Tree,
            Category::Shrub,
            Category::Grass,
            Category::Perennial,
            Category::Furniture,
        ] {
            let a = asset(c, [4.0, 6.0, 4.0]);
            for (m, _) in parts(Some(&a)) {
                assert!(!m.indices.is_empty());
                assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
                let top = m.positions.iter().map(|p| p[1]).fold(0.0, f32::max);
                assert!(top <= 6.001, "{c:?} {top}");
                assert!(
                    m.positions
                        .iter()
                        .all(|p| p[0].abs() <= 2.001 && p[2].abs() <= 2.001)
                );
            }
        }
    }
}
