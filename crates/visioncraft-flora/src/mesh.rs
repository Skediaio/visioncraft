//! Mesh building blocks shared by the generators: tapered tubes for wood, foliage cards with
//! bent normals, and cell UVs into the leaf atlas.

use crate::math::{Rng, TAU, UP, V3, cos, sin, v3};
use crate::texture::{ATLAS_CELLS, CELL_MARGIN, CELL_PX};
use crate::{Material, Part, PlantMesh};

/// A skeleton node: position, radius, wind weight.
#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub p: V3,
    pub r: f32,
    pub w: f32,
}

/// Where foliage normals bend towards. See [`Builder::card`].
#[derive(Clone, Copy, Debug)]
pub enum Outward {
    /// Gradient of an ellipsoid (tree crowns, shrubs).
    Ellipsoid { centre: V3, radii: V3 },
    /// Away from a vertical axis through `centre`, plus `up` towards the sky (grass, rosettes).
    Axis { centre: V3, up: f32 },
    /// A fixed direction (walls, ground cover).
    Dir(V3),
}

impl Outward {
    pub fn at(&self, p: V3) -> V3 {
        match *self {
            Outward::Ellipsoid { centre, radii } => {
                let d = p - centre;
                v3(
                    d.x / (radii.x * radii.x),
                    d.y / (radii.y * radii.y),
                    d.z / (radii.z * radii.z),
                )
                .norm()
            }
            Outward::Axis { centre, up } => {
                let d = p - centre;
                (v3(d.x, 0.0, d.z).norm_or_zero() + UP * up).norm()
            }
            Outward::Dir(d) => d,
        }
    }
}

impl V3 {
    fn norm_or_zero(self) -> V3 {
        let l = self.len();
        if l > 1e-6 {
            self * (1.0 / l)
        } else {
            V3::default()
        }
    }
}

/// A foliage or flower card: a quad, optionally bent along its length into `segments` pieces.
#[derive(Clone, Copy, Debug)]
pub struct Card {
    /// Centre of the card's bottom edge (where the leaf/twig attaches).
    pub base: V3,
    /// Unit direction from bottom edge to top edge.
    pub up: V3,
    /// Unit direction across the card.
    pub side: V3,
    pub length: f32,
    pub width: f32,
    /// Atlas cell.
    pub cell: u8,
    pub material: Material,
    /// Wind weight at the base and at the top edge.
    pub wind: (f32, f32),
    pub phase: f32,
    /// Downward bend in radians over the full length (0 = flat quad).
    pub bend: f32,
    pub segments: u8,
    /// How much of the bent normal comes from the outward field (0..1).
    pub outward_mix: f32,
}

#[derive(Default)]
pub struct Builder {
    pos: Vec<[f32; 3]>,
    nrm: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    wind: Vec<[f32; 2]>,
    idx: [Vec<u32>; 3],
}

fn mat_index(m: Material) -> usize {
    match m {
        Material::Bark => 0,
        Material::Foliage => 1,
        Material::Flower => 2,
    }
}

/// UV of local card coordinates (s across 0..1, t base-to-tip 0..1) in atlas cell `cell`.
pub fn cell_uv(cell: u8, s: f32, t: f32) -> [f32; 2] {
    let n = ATLAS_CELLS as f32;
    let inset = CELL_MARGIN as f32 / CELL_PX as f32;
    let cx = (cell as u32 % ATLAS_CELLS) as f32;
    let cy = (cell as u32 / ATLAS_CELLS) as f32;
    let s = inset + s.clamp(0.0, 1.0) * (1.0 - 2.0 * inset);
    let t = inset + t.clamp(0.0, 1.0) * (1.0 - 2.0 * inset);
    [(cx + s) / n, (cy + 1.0 - t) / n]
}

impl Builder {
    pub fn vert(&mut self, p: V3, n: V3, uv: [f32; 2], wind: [f32; 2]) -> u32 {
        self.pos.push(p.arr());
        self.nrm.push(n.norm().arr());
        self.uv.push([uv[0].clamp(0.0, 1.0), uv[1].clamp(0.0, 1.0)]);
        self.wind.push([wind[0].clamp(0.0, 1.0), wind[1]]);
        (self.pos.len() - 1) as u32
    }

    pub fn tri(&mut self, m: Material, a: u32, b: u32, c: u32) {
        self.idx[mat_index(m)].extend_from_slice(&[a, b, c]);
    }

    pub fn quad(&mut self, m: Material, a: u32, b: u32, c: u32, d: u32) {
        self.tri(m, a, b, c);
        self.tri(m, a, c, d);
    }

    /// Bounds of the vertices from `start` on.
    pub fn bounds_from(&self, start: usize) -> (V3, V3) {
        let mut lo = v3(f32::MAX, f32::MAX, f32::MAX);
        let mut hi = v3(f32::MIN, f32::MIN, f32::MIN);
        for p in &self.pos[start..] {
            lo = v3(lo.x.min(p[0]), lo.y.min(p[1]), lo.z.min(p[2]));
            hi = v3(hi.x.max(p[0]), hi.y.max(p[1]), hi.z.max(p[2]));
        }
        (lo, hi)
    }

    /// Scale vertices from `start` on about the origin so each horizontal half-axis reaches
    /// `half_w` (per side, so the base stays at the origin and the bounds are symmetric) and
    /// the top reaches `h`. Factors are clamped to [0.5, 2].
    pub fn fit_from(&mut self, start: usize, half_w: f32, h: f32) {
        let (lo, hi) = self.bounds_from(start);
        let k = |ext: f32| (half_w / ext.max(1e-4)).clamp(0.5, 2.0);
        let (kxp, kxn, kzp, kzn) = (k(hi.x), k(-lo.x), k(hi.z), k(-lo.z));
        let ky = (h / hi.y.max(1e-4)).clamp(0.5, 2.0);
        for p in &mut self.pos[start..] {
            p[0] *= if p[0] > 0.0 { kxp } else { kxn };
            p[1] *= ky;
            p[2] *= if p[2] > 0.0 { kzp } else { kzn };
        }
    }

    pub fn tris(&self) -> usize {
        self.idx.iter().map(|v| v.len() / 3).sum()
    }

    /// Triangles a [`Builder::tube`] call would add.
    pub fn tube_cost(nodes: usize, sides: u32) -> usize {
        if nodes < 2 {
            0
        } else {
            (nodes - 2) * 2 * sides as usize + sides as usize
        }
    }

    /// A tapered generalised cylinder through `nodes` with `sides` facets, ending in an apex at
    /// the last node. The bark V coordinate repeats about once per base circumference; each
    /// segment gets its own pair of rings so UVs stay inside [0,1] without a wrapping sampler.
    pub fn tube(&mut self, nodes: &[Node], sides: u32, phase: f32) {
        if nodes.len() < 2 {
            return;
        }
        let circ = (TAU * nodes[0].r).max(0.01);
        let total: f32 = nodes.windows(2).map(|w| (w[1].p - w[0].p).len()).sum();
        let seg_len = total / (nodes.len() - 1) as f32;
        let per_repeat = ((circ / seg_len.max(1e-4)).round() as usize).max(1); // segments per texture repeat
        let dir = |i: usize| -> V3 {
            let a = nodes[i.saturating_sub(1)].p;
            let b = nodes[(i + 1).min(nodes.len() - 1)].p;
            (b - a).norm()
        };
        // Parallel-transported frame.
        let mut side = dir(0).perp();
        let mut frames = Vec::with_capacity(nodes.len());
        for i in 0..nodes.len() {
            let d = dir(i);
            side = (side - d * side.dot(d)).norm();
            frames.push((d, side, d.cross(side)));
        }
        let ring = |b: &mut Builder, i: usize, v: f32| -> u32 {
            let (_, s, c) = frames[i];
            let first = b.pos.len() as u32;
            for k in 0..=sides {
                let a = TAU * k as f32 / sides as f32;
                let n = s * cos(a) + c * sin(a);
                b.vert(
                    nodes[i].p + n * nodes[i].r,
                    n,
                    [k as f32 / sides as f32, v],
                    [nodes[i].w, phase],
                );
            }
            first
        };
        let last = nodes.len() - 1;
        for j in 0..last {
            let v0 = (j % per_repeat) as f32 / per_repeat as f32;
            let v1 = v0 + 1.0 / per_repeat as f32;
            let r0 = ring(self, j, 1.0 - v0);
            if j + 1 < last {
                let r1 = ring(self, j + 1, 1.0 - v1);
                for k in 0..sides {
                    self.quad(Material::Bark, r0 + k, r0 + k + 1, r1 + k + 1, r1 + k);
                }
            } else {
                // Apex: one tip vertex per facet so each keeps its own U.
                let (d, _, _) = frames[last];
                for k in 0..sides {
                    let tip = self.vert(
                        nodes[last].p,
                        d,
                        [(k as f32 + 0.5) / sides as f32, 1.0 - v1],
                        [nodes[last].w, phase],
                    );
                    self.tri(Material::Bark, r0 + k, r0 + k + 1, tip);
                }
            }
        }
    }

    /// Emit a card. Normals are bent: `normalize(mix * outward(p) + (1 - mix) * card_normal)`,
    /// with the card normal flipped to the outward side. This is the usual trick that makes a
    /// cloud of flat cards shade like one volume (a crown, a clump) instead of as flat facets.
    pub fn card(&mut self, c: &Card, out: &Outward) {
        let segs = c.segments.max(1) as usize;
        let mut face_n = c.side.cross(c.up).norm();
        let mid = c.base + c.up * (c.length * 0.5);
        if face_n.dot(out.at(mid)) < 0.0 {
            face_n = -face_n;
        }
        let mut p = c.base;
        let mut dirv = c.up;
        let step = c.length / segs as f32;
        let bend_step = c.bend / segs as f32;
        let mut prev: Option<(u32, u32)> = None;
        for i in 0..=segs {
            let t = i as f32 / segs as f32;
            let w = c.wind.0 + (c.wind.1 - c.wind.0) * t;
            let n_card = face_n;
            let half = c.side * (c.width * 0.5);
            let make = |b: &mut Builder, q: V3, s: f32| {
                let o = out.at(q);
                let n = (o * c.outward_mix + n_card * (1.0 - c.outward_mix)).norm();
                b.vert(q, n, cell_uv(c.cell, s, t), [w, c.phase])
            };
            let a = make(self, p - half, 0.0);
            let b = make(self, p + half, 1.0);
            if let Some((pa, pb)) = prev {
                self.quad(c.material, pa, pb, b, a);
            }
            prev = Some((a, b));
            if i < segs {
                p += dirv * step;
                if bend_step != 0.0 {
                    // Bend about the card's side axis, whichever way takes the tip down (droop).
                    let a = dirv.rotate(c.side, bend_step);
                    let b = dirv.rotate(c.side, -bend_step);
                    dirv = if a.y < b.y { a } else { b };
                }
            }
        }
    }

    /// A pair of crossed cards (4 triangles), for flowers and plumes seen from any side.
    pub fn crossed(&mut self, c: &Card, out: &Outward) {
        self.card(c, out);
        let side2 = c.up.cross(c.side).norm();
        self.card(&Card { side: side2, ..*c }, out);
    }

    pub fn finish(self) -> PlantMesh {
        let mut mesh = PlantMesh {
            positions: self.pos,
            normals: self.nrm,
            uvs: self.uv,
            wind: self.wind,
            ..Default::default()
        };
        for (m, list) in [Material::Bark, Material::Foliage, Material::Flower]
            .into_iter()
            .zip(self.idx)
        {
            if list.is_empty() {
                continue;
            }
            let start = mesh.indices.len() as u32;
            mesh.indices.extend_from_slice(&list);
            mesh.parts.push(Part {
                material: m,
                index_range: start..mesh.indices.len() as u32,
            });
        }
        mesh
    }
}

/// A deterministic permutation of `0..n` (Fisher-Yates), used to pick the same subset of
/// foliage points for every level of detail: LOD k takes the first N_k entries.
pub fn shuffled(n: usize, rng: &mut Rng) -> Vec<usize> {
    let mut v: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        let j = rng.below(i + 1);
        v.swap(i, j);
    }
    v
}

/// Random unit vector.
pub fn rand_dir(rng: &mut Rng) -> V3 {
    loop {
        let v = v3(rng.sym(), rng.sym(), rng.sym());
        let l = v.dot(v);
        if l > 1e-4 && l <= 1.0 {
            return v * (1.0 / l.sqrt());
        }
    }
}
