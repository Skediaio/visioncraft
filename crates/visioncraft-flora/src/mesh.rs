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
    /// A tree crown's leaves: the ellipsoid's gradient with its vertical part halved. A crown
    /// shaded as one ellipsoid reads as a pale cap (its top faces the high sun squarely) over a
    /// black underside; real leaves face every way, most of them sideways.
    Crown { centre: V3, radii: V3 },
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
            Outward::Crown { centre, radii } => {
                let o = Outward::Ellipsoid { centre, radii }.at(p);
                v3(o.x, o.y * 0.5, o.z).norm()
            }
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

/// Root buttresses at a trunk's foot: the radius swells by up to `amount` in `lobes` ridges
/// round the stem (where the main roots leave it), fading out by `height` metres up.
#[derive(Clone, Copy, Debug)]
pub struct Buttress {
    pub height: f32,
    pub amount: f32,
    pub lobes: f32,
    pub phase: f32,
}

impl Buttress {
    /// The swelling round the stem at angle `a` (radians) and its derivative by `a`.
    fn lobe(&self, a: f32) -> (f32, f32) {
        let x = self.lobes * a + self.phase;
        let c = cos(x);
        if c <= 0.0 {
            return (0.0, 0.0);
        }
        (c * c * c, -3.0 * c * c * sin(x) * self.lobes)
    }

    /// How much of the swelling there is at height `y` and its derivative by `y`.
    fn rise(&self, y: f32) -> (f32, f32) {
        let t = 1.0 - y / self.height.max(1e-3);
        if t <= 0.0 {
            return (0.0, 0.0);
        }
        let t = t.min(1.0);
        (self.amount * t * t, -2.0 * self.amount * t / self.height.max(1e-3))
    }
}

#[derive(Default)]
pub struct Builder {
    pos: Vec<[f32; 3]>,
    nrm: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    wind: Vec<[f32; 2]>,
    idx: [Vec<u32>; 4],
}

fn mat_index(m: Material) -> usize {
    match m {
        Material::Bark => 0,
        Material::Foliage => 1,
        Material::Flower => 2,
        Material::Twigs => 3,
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
        self.push(p, n, [uv[0].clamp(0.0, 1.0), uv[1].clamp(0.0, 1.0)], wind)
    }

    /// A vertex with its UV as given: bark, whose texture repeats.
    fn push(&mut self, p: V3, n: V3, uv: [f32; 2], wind: [f32; 2]) -> u32 {
        self.pos.push(p.arr());
        self.nrm.push(n.norm().arr());
        self.uv.push(uv);
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
    /// the top reaches `h`. Factors are clamped to [0.3, 2].
    pub fn fit_from(&mut self, start: usize, half_w: f32, h: f32) {
        let (lo, hi) = self.bounds_from(start);
        let k = |ext: f32| (half_w / ext.max(1e-4)).clamp(0.3, 2.0);
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
    /// the last node. The bark texture goes once round and repeats along every local girth (its
    /// V runs past 1: bark is drawn with a repeating sampler).
    pub fn tube(&mut self, nodes: &[Node], sides: u32, phase: f32) {
        self.tube_as(nodes, sides, phase, None, None);
    }

    /// A [`Builder::tube`] for a trunk standing on the ground, its foot swelling into root
    /// buttresses.
    pub fn trunk(&mut self, nodes: &[Node], sides: u32, phase: f32, b: Buttress) {
        self.tube_as(nodes, sides, phase, None, Some(b));
    }

    /// A [`Builder::tube`] of fine twigs: drawn with [`Material::Twigs`], every vertex on the
    /// solid twig-coloured atlas cell 7 of grown trees (fine twigs are darker and greyer than
    /// the trunk's bark, a birch's most of all).
    pub fn twig_tube(&mut self, nodes: &[Node], sides: u32, phase: f32) {
        self.tube_as(nodes, sides, phase, Some(cell_uv(7, 0.5, 0.5)), None);
    }

    fn tube_as(
        &mut self,
        nodes: &[Node],
        sides: u32,
        phase: f32,
        solid: Option<[f32; 2]>,
        buttress: Option<Buttress>,
    ) {
        let material = if solid.is_some() {
            Material::Twigs
        } else {
            Material::Bark
        };
        if nodes.len() < 2 {
            return;
        }
        // Bark: U goes once round, V along the branch at the local girth's scale, so the photo
        // keeps its aspect where the branch tapers or flares (V was one base girth per repeat:
        // a flared trunk's bark stretched up its whole length). V runs on past 1: the bark
        // texture repeats.
        let mut along = vec![0.0f32; nodes.len()];
        for i in 1..nodes.len() {
            let r = ((nodes[i - 1].r + nodes[i].r) * 0.5).max(1e-3);
            along[i] = along[i - 1] + (nodes[i].p - nodes[i - 1].p).len() / (TAU * r);
        }
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
        let last = nodes.len() - 1;
        // One ring per node, shared by the segments either side: normals and UVs run on
        // unbroken. A ring's normals lean with the taper (towards the thin end), so a flare
        // or a swelling shades as one.
        let ring = |b: &mut Builder, i: usize| -> u32 {
            let (d, s, c) = frames[i];
            let (i0, i1) = (i.saturating_sub(1), (i + 1).min(last));
            let ds = (nodes[i1].p - nodes[i0].p).len().max(1e-4);
            let slope = ((nodes[i0].r - nodes[i1].r) / ds).clamp(-2.0, 2.0);
            let first = b.pos.len() as u32;
            let r = nodes[i].r;
            for k in 0..=sides {
                let a = TAU * k as f32 / sides as f32;
                let n = s * cos(a) + c * sin(a);
                let uv = solid.unwrap_or([k as f32 / sides as f32, 1.0 - along[i]]);
                let (mut rk, mut nk) = (r, n + d * slope);
                if let Some(bt) = buttress {
                    let (lobe, dlobe) = bt.lobe(a);
                    let (g, dg) = bt.rise(nodes[i].p.y);
                    rk = r * (1.0 + lobe * g);
                    // The lobe's sides face round it; its top slopes into the stem.
                    let round = c * cos(a) - s * sin(a);
                    nk = nk - round * (dlobe * g) - d * (lobe * dg * r);
                }
                b.push(nodes[i].p + n * rk, nk.norm(), uv, [nodes[i].w, phase]);
            }
            first
        };
        let mut r0 = ring(self, 0);
        for j in 0..last {
            if j + 1 < last {
                let r1 = ring(self, j + 1);
                for k in 0..sides {
                    self.quad(material, r0 + k, r0 + k + 1, r1 + k + 1, r1 + k);
                }
                r0 = r1;
            } else {
                // Apex: one tip vertex per facet so each keeps its own U.
                let (d, _, _) = frames[last];
                for k in 0..sides {
                    let tip = self.push(
                        nodes[last].p,
                        d,
                        solid.unwrap_or([(k as f32 + 0.5) / sides as f32, 1.0 - along[last]]),
                        [nodes[last].w, phase],
                    );
                    self.tri(material, r0 + k, r0 + k + 1, tip);
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
        for (m, list) in Material::ALL.into_iter().zip(self.idx) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn node(y: f32, r: f32) -> Node {
        Node {
            p: v3(0.0, y, 0.0),
            r,
            w: 0.0,
        }
    }

    /// Bark keeps its aspect: V runs one repeat per local girth along the branch, unbroken
    /// where segments meet, and past 1.
    #[test]
    fn bark_runs_along_the_local_girth() {
        let mut b = Builder::default();
        b.tube(&[node(0.0, 0.2), node(1.0, 0.1), node(2.0, 0.1), node(3.0, 0.05)], 8, 0.0);
        let m = b.finish();
        let v_at = |y: f32| {
            let i = m.positions.iter().position(|p| (p[1] - y).abs() < 1e-4).unwrap();
            1.0 - m.uvs[i][1]
        };
        let girth = |r: f32| TAU * r;
        assert!((v_at(1.0) - 1.0 / girth(0.15)).abs() < 1e-3);
        assert!((v_at(2.0) - v_at(1.0) - 1.0 / girth(0.1)).abs() < 1e-3);
        assert!(v_at(2.0) > 1.0);
    }

    /// Root buttresses swell the foot in ridges and are gone above their height.
    #[test]
    fn buttresses_swell_the_foot_only() {
        let mut b = Builder::default();
        let bt = Buttress {
            height: 0.9,
            amount: 0.4,
            lobes: 5.0,
            phase: 0.3,
        };
        b.trunk(&[node(0.0, 0.3), node(0.5, 0.3), node(1.0, 0.3), node(3.0, 0.2)], 24, 0.0, bt);
        let m = b.finish();
        let radii = |y: f32| -> Vec<f32> {
            m.positions
                .iter()
                .filter(|p| (p[1] - y).abs() < 1e-4)
                .map(|p| (p[0] * p[0] + p[2] * p[2]).sqrt())
                .collect()
        };
        let foot = radii(0.0);
        let max = foot.iter().fold(0.0f32, |a, &r| a.max(r));
        let min = foot.iter().fold(f32::MAX, |a, &r| a.min(r));
        assert!(max > 0.3 * 1.15 && (min - 0.3).abs() < 1e-3, "{min} .. {max}");
        assert!(radii(1.0).iter().all(|r| (r - 0.3).abs() < 1e-3));
    }
}
