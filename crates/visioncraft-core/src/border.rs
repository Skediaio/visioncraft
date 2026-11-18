//! Denser borders: a layered perennial planting that closes up, built from a curated species
//! list per style.
//!
//! A border is an [`AreaScatter`](crate::planting::AreaScatter) with a [`Border`] on it. Its
//! plants are placed like this, deterministically from the generator's seed:
//!
//! - **Layers by height.** The border has a front (the side the viewer stands: `facing` points
//!   from the back to the front) and a back. A species is chosen for a spot by how close its
//!   mature height (`growth`) is to the height wanted there: `height_range_m` `[front, back]`,
//!   linear across the depth. Short plants end up at the front, tall ones at the back.
//! - **Drifts.** The ground is cut into elongated cells along the border (a jittered grid, a
//!   Voronoi diagram stretched `drift_m` long and about a metre deep). Each cell holds one
//!   species, so plants of a kind stand in a drift of a few to a dozen; near the cell's edge,
//!   a matrix species (a cranesbill, a sedge, a lady's mantle) fills between the drifts.
//! - **Closes up.** Plants stand at a density from their own mature width `w`: `closure / w^2`
//!   per m2 (1.3 by default: crowns touch and overlap at maturity). The growth curves make a
//!   young border open and a mature one closed: the placements are the same at every age.
//!
//! The garden's own height variation and the drift layout depend on the seed only.

use crate::growth::{Grower, growth_of};
use crate::planting::{
    AreaScatter, Evaluation, MAX_PLACEMENTS, Placement, Rng, bounds, mix, point_in_polygon,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The styles `planting.add_border` knows.
pub const STYLES: &[&str] = &["cottage", "prairie", "shade", "dry"];

/// A species of a border, with how often it is chosen and whether it fills between the drifts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BorderSpecies {
    /// `flora:<id>` or a custom species id.
    pub asset: String,
    #[serde(default = "one")]
    pub weight: f32,
    /// A matrix species grows between the drifts (low, spreading plants and grasses).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub matrix: bool,
}

fn one() -> f32 {
    1.0
}

fn default_closure() -> f32 {
    1.3
}

fn default_drift() -> f32 {
    2.4
}

/// The layered, drifted layout of an area scatter (see the module docs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Border {
    /// `cottage`, `prairie`, `shade` or `dry`, or what the planting was called by hand.
    pub style: String,
    /// Unit direction `[x, z]` from the back of the border to its front (towards the viewer).
    pub facing: [f32; 2],
    /// Mature plant height (m) wanted at the front and at the back.
    pub height_range_m: [f32; 2],
    pub species: Vec<BorderSpecies>,
    /// Plants per m2 = `closure / mature width^2` for each species.
    #[serde(default = "default_closure")]
    pub closure: f32,
    /// Typical length of a drift (m) along the border.
    #[serde(default = "default_drift")]
    pub drift_m: f32,
}

impl Border {
    pub fn validate(&self) -> Result<(), String> {
        let ok = |c: bool, why: &str| if c { Ok(()) } else { Err(why.to_string()) };
        ok(
            self.facing.iter().all(|v| v.is_finite())
                && self.facing[0].hypot(self.facing[1]) > 1e-4,
            "border facing: a direction [x, z], not zero",
        )?;
        let [lo, hi] = self.height_range_m;
        ok(
            lo.is_finite() && hi.is_finite() && lo > 0.0 && lo <= hi && hi <= 10.0,
            "border height_range_m: 0 < front <= back <= 10",
        )?;
        ok(
            (0.3..=3.0).contains(&self.closure),
            "border closure: 0.3..3",
        )?;
        ok(
            (1.0..=10.0).contains(&self.drift_m),
            "border drift_m: 1..10",
        )?;
        ok(
            self.species
                .iter()
                .all(|s| s.weight.is_finite() && s.weight >= 0.0)
                && self.species.iter().any(|s| s.weight > 0.0),
            "a border needs at least one species with weight above zero",
        )
    }
}

fn sp(id: &str, weight: f32, matrix: bool) -> BorderSpecies {
    BorderSpecies {
        asset: format!("flora:{id}"),
        weight,
        matrix,
    }
}

/// The curated species of a style and its default `height_range_m` (front, back), `None` for an
/// unknown style. Every species is a built-in one with growth data. Heights are the mature
/// heights of `growth`, so a range picks what stands where.
pub fn style(style: &str) -> Option<(Vec<BorderSpecies>, [f32; 2])> {
    let s = match style {
        // A sunny mixed border of the cottage garden: spires at the back, daisies and sages in
        // the middle, cranesbill and catmint along the front.
        "cottage" => (
            vec![
                sp("delphinium-elatum", 0.6, false),
                sp("phlox-paniculata", 1.4, false),
                sp("lupinus-polyphyllus", 1.2, false),
                sp("anemone-hupehensis", 0.8, false),
                sp("paeonia-lactiflora", 1.0, false),
                sp("echinacea-purpurea", 1.2, false),
                sp("rudbeckia-fulgida", 1.2, false),
                sp("iris-germanica", 0.8, false),
                sp("salvia-nemorosa", 1.4, false),
                sp("lavandula-angustifolia", 1.0, false),
                sp("nepeta-faassenii", 1.0, true),
                sp("geranium-rozanne", 1.4, true),
                sp("alchemilla-mollis", 1.2, true),
            ],
            [0.4, 1.6],
        ),
        // Grasses and late daisies in big drifts, open and sunny.
        "prairie" => (
            vec![
                sp("miscanthus-sinensis", 0.8, false),
                sp("calamagrostis-acutiflora", 1.2, false),
                sp("panicum-virgatum", 1.0, false),
                sp("anemone-hupehensis", 0.8, false),
                sp("phlox-paniculata", 0.8, false),
                sp("echinacea-purpurea", 1.6, false),
                sp("rudbeckia-fulgida", 1.6, false),
                sp("salvia-nemorosa", 1.0, false),
                sp("pennisetum-alopecuroides", 1.0, false),
                sp("sedum-herbstfreude", 1.2, false),
                sp("carex-oshimensis", 1.0, true),
                sp("festuca-glauca", 1.0, true),
                sp("geranium-rozanne", 1.0, true),
            ],
            [0.35, 1.8],
        ),
        // Foliage and quiet flowers for partial and deep shade.
        "shade" => (
            vec![
                sp("dryopteris-filix-mas", 1.0, false),
                sp("anemone-hupehensis", 1.0, false),
                sp("hydrangea-macrophylla", 0.6, false),
                sp("hosta", 1.4, false),
                sp("astilbe-arendsii", 1.4, false),
                sp("helleborus", 1.0, false),
                sp("heuchera", 1.0, false),
                sp("bergenia-cordifolia", 0.8, false),
                sp("hakonechloa-macra", 1.0, true),
                sp("alchemilla-mollis", 1.2, true),
                sp("pachysandra-terminalis", 1.0, true),
                sp("vinca-minor", 0.8, true),
            ],
            [0.2, 1.2],
        ),
        // Sun, lean soil and little water: lavender, sages, stonecrops and fine grasses.
        "dry" => (
            vec![
                sp("miscanthus-sinensis", 0.5, false),
                sp("calamagrostis-acutiflora", 0.8, false),
                sp("iris-germanica", 1.0, false),
                sp("allium-purple-sensation", 1.0, false),
                sp("echinacea-purpurea", 1.0, false),
                sp("rudbeckia-fulgida", 0.8, false),
                sp("salvia-nemorosa", 1.4, false),
                sp("lavandula-angustifolia", 1.6, false),
                sp("sedum-herbstfreude", 1.4, false),
                sp("pennisetum-alopecuroides", 0.8, false),
                sp("nepeta-faassenii", 1.0, true),
                sp("festuca-glauca", 1.2, true),
                sp("thymus-serpyllum", 1.0, true),
            ],
            [0.25, 1.4],
        ),
        _ => return None,
    };
    Some(s)
}

/// Whether a species fills between drifts when the species list is given by hand: ground cover,
/// and low spreading plants.
pub fn is_matrix_by_size(asset: &str) -> bool {
    let g = growth_of(asset, None);
    g.grower == Grower::GroundCover
        || (g.mature_height_m <= 0.5 && g.mature_width_m >= g.mature_height_m)
}

/// The direction from the back of a border to its front, from what stands round it:
/// `backs` are polylines (closed or not, as segment lists) of walls, buildings and hedges. The
/// back is the nearest of them when within 2.5 m of the outline, and the front points away from
/// it, along the normal of the nearest segment. Without one: the outward normal of the
/// outline's longest edge.
pub fn derive_facing(outline: &[[f32; 2]], backs: &[Vec<([f32; 2], [f32; 2])>]) -> [f32; 2] {
    let n = outline.len();
    let centre = [
        outline.iter().map(|p| p[0]).sum::<f32>() / n as f32,
        outline.iter().map(|p| p[1]).sum::<f32>() / n as f32,
    ];
    let mut best: Option<(f32, [f32; 2])> = None;
    for seg in backs.iter().flatten() {
        // Distance from the outline's vertices and edge midpoints to the segment.
        let probes = outline.iter().copied().chain((0..n).map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
        }));
        let d = probes
            .map(|p| distance_to_segment(p, seg.0, seg.1))
            .fold(f32::INFINITY, f32::min);
        if d <= 2.5 && best.is_none_or(|b| d < b.0) {
            let e = [seg.1[0] - seg.0[0], seg.1[1] - seg.0[1]];
            let len = e[0].hypot(e[1]);
            if len > 1e-4 {
                let mut nrm = [-e[1] / len, e[0] / len];
                let mid = [(seg.0[0] + seg.1[0]) / 2.0, (seg.0[1] + seg.1[1]) / 2.0];
                if nrm[0] * (centre[0] - mid[0]) + nrm[1] * (centre[1] - mid[1]) < 0.0 {
                    nrm = [-nrm[0], -nrm[1]];
                }
                best = Some((d, nrm));
            }
        }
    }
    if let Some((_, f)) = best {
        return f;
    }
    let longest = (0..n)
        .max_by(|&i, &j| edge_len(outline, i).total_cmp(&edge_len(outline, j)))
        .unwrap_or(0);
    edge_outward_normal(outline, longest)
}

fn edge_len(outline: &[[f32; 2]], i: usize) -> f32 {
    let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
    (b[0] - a[0]).hypot(b[1] - a[1])
}

fn signed_area(outline: &[[f32; 2]]) -> f32 {
    (0..outline.len())
        .map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f32>()
        / 2.0
}

/// The unit normal of edge `i` pointing out of the polygon.
pub fn edge_outward_normal(outline: &[[f32; 2]], i: usize) -> [f32; 2] {
    let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
    let e = [b[0] - a[0], b[1] - a[1]];
    let len = e[0].hypot(e[1]).max(1e-6);
    // Counter-clockwise (positive area): the right of the direction of travel is outside.
    let right = [e[1] / len, -e[0] / len];
    if signed_area(outline) >= 0.0 {
        right
    } else {
        [-right[0], -right[1]]
    }
}

/// The front direction of a border from the edges (by index) that face the viewer: the mean of
/// their outward normals, weighted by length.
pub fn facing_of_edges(outline: &[[f32; 2]], edges: &[usize]) -> Option<[f32; 2]> {
    let mut sum = [0.0f32; 2];
    for &i in edges {
        let nrm = edge_outward_normal(outline, i);
        let l = edge_len(outline, i);
        sum[0] += nrm[0] * l;
        sum[1] += nrm[1] * l;
    }
    let len = sum[0].hypot(sum[1]);
    (len > 1e-4).then(|| [sum[0] / len, sum[1] / len])
}

fn distance_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let l2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * ab[0]).hypot(p[1] - a[1] - t * ab[1])
}

/// Plants accepted so far (position, mature width) by cell.
type Taken = HashMap<(i32, i32), Vec<([f32; 2], f32)>>;

/// What the layout needs of one species.
struct Info {
    asset: String,
    width: f32,
    height: f32,
    weight: f32,
    matrix: bool,
}

/// One drift's centre and what grows in it and in the matrix round it.
struct Site {
    u: f32,
    v: f32,
    drift: Option<usize>,
    matrix: Option<usize>,
}

struct Layout<'a> {
    seed: u64,
    infos: &'a [Info],
    /// Across and along the border: `a` runs along it, `f` towards the front.
    a: [f32; 2],
    f: [f32; 2],
    vmin: f32,
    vmax: f32,
    cu: f32,
    cv: f32,
    height_range: [f32; 2],
}

/// How much a species of mature height `h` suits a spot where `target` is wanted.
fn fit(h: f32, target: f32, sigma: f32) -> f32 {
    let d = (h - target) / sigma;
    (-0.5 * d * d).exp().max(0.01)
}

impl Layout<'_> {
    fn uv(&self, p: [f32; 2]) -> (f32, f32) {
        (
            p[0] * self.a[0] + p[1] * self.a[1],
            p[0] * self.f[0] + p[1] * self.f[1],
        )
    }

    /// 0 at the front of the border, 1 at its back.
    fn depth(&self, v: f32) -> f32 {
        ((self.vmax - v) / (self.vmax - self.vmin)).clamp(0.0, 1.0)
    }

    fn target(&self, t: f32) -> f32 {
        self.height_range[0] + t * (self.height_range[1] - self.height_range[0])
    }

    fn pick(&self, matrix: bool, t: f32, r: f32) -> Option<usize> {
        let [lo, hi] = self.height_range;
        let sigma = (0.16 * (hi - lo)).max(0.1);
        let target = self.target(t);
        let scores: Vec<(usize, f32)> = self
            .infos
            .iter()
            .enumerate()
            .filter(|(_, s)| s.matrix == matrix && s.weight > 0.0)
            .map(|(i, s)| (i, s.weight * fit(s.height, target, sigma)))
            .collect();
        let total: f32 = scores.iter().map(|s| s.1).sum();
        let mut x = r * total;
        for &(i, w) in &scores {
            if x < w {
                return Some(i);
            }
            x -= w;
        }
        scores.last().map(|s| s.0)
    }

    fn site(&self, iu: i64, iv: i64) -> Site {
        let mut rng = Rng::new(mix(mix(self.seed ^ 0x00B0_4DE4, iu), iv));
        let u = (iu as f32 + 0.2 + 0.6 * rng.f32()) * self.cu;
        let v = self.vmin + (iv as f32 + 0.2 + 0.6 * rng.f32()) * self.cv;
        let t = self.depth(v);
        let (r_drift, r_matrix) = (rng.f32(), rng.f32());
        Site {
            u,
            v,
            drift: self.pick(false, t, r_drift),
            matrix: self.pick(true, t, r_matrix),
        }
    }

    /// The species at `p`: the drift of the nearest site, or the matrix near the border between
    /// two drifts.
    fn species_at(&self, p: [f32; 2]) -> Option<usize> {
        let (u, v) = self.uv(p);
        let iu = (u / self.cu).floor() as i64;
        let iv = ((v - self.vmin) / self.cv).floor() as i64;
        let (mut d1, mut d2) = (f32::INFINITY, f32::INFINITY);
        let mut near: Option<Site> = None;
        for du in -1..=1 {
            for dv in -1..=1 {
                let s = self.site(iu + du, iv + dv);
                let (x, y) = ((u - s.u) / self.cu, (v - s.v) / self.cv);
                let d = x * x + y * y;
                if d < d1 {
                    d2 = d1;
                    d1 = d;
                    near = Some(s);
                } else if d < d2 {
                    d2 = d;
                }
            }
        }
        let near = near?;
        // Less matrix towards the back, where the tall plants close up.
        let thr = 0.78 + 0.14 * self.depth(v);
        let between = d2 > 0.0 && (d1 / d2).sqrt() > thr;
        match (near.drift, near.matrix) {
            (Some(d), Some(m)) => Some(if between { m } else { d }),
            (Some(d), None) => Some(d),
            (None, m) => m,
        }
    }
}

/// The placements of an area scatter with a border. Deterministic by `g.seed`.
pub(crate) fn evaluate(
    g: &AreaScatter,
    b: &Border,
    ground: &dyn Fn(f32, f32) -> Option<f32>,
) -> Evaluation {
    let infos: Vec<Info> = b
        .species
        .iter()
        .map(|s| {
            let gr = growth_of(&s.asset, None);
            Info {
                asset: s.asset.clone(),
                width: gr.mature_width_m.max(0.1),
                height: gr.mature_height_m,
                weight: s.weight,
                matrix: s.matrix,
            }
        })
        .collect();
    let len = b.facing[0].hypot(b.facing[1]);
    let f = [b.facing[0] / len, b.facing[1] / len];
    let a = [-f[1], f[0]];
    let vs: Vec<f32> = g
        .outline
        .iter()
        .map(|p| p[0] * f[0] + p[1] * f[1])
        .collect();
    let vmin = vs.iter().copied().fold(f32::INFINITY, f32::min);
    let vmax = vs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let span = (vmax - vmin).max(0.2);
    // About 0.8 m per row of drifts across the border, at least one row.
    let rows = (span / 0.8).round().max(1.0);
    let layout = Layout {
        seed: g.seed,
        infos: &infos,
        a,
        f,
        vmin,
        vmax: vmin + span,
        cu: b.drift_m,
        cv: span / rows,
        height_range: b.height_range_m,
    };
    let bb = bounds(g.outline.iter().copied());
    // Large plants first: they keep their room, the small ones fill round them.
    let mut order: Vec<usize> = (0..infos.len()).collect();
    order.sort_by(|&i, &j| infos[j].width.total_cmp(&infos[i].width).then(i.cmp(&j)));
    let largest = infos.iter().map(|s| s.width).fold(0.1, f32::max);
    // Accepted plants in a hash of cells as wide as the widest plant.
    let cell = largest.max(0.5);
    let mut taken: Taken = HashMap::new();
    let mut out: Vec<Placement> = Vec::new();
    let mut visited = 0usize;
    'species: for &k in &order {
        let s = &infos[k];
        if s.weight <= 0.0 {
            continue;
        }
        // A hexagonal lattice with `closure / w^2` plants per m2, shifted by species and
        // jittered, so the crowns of a drift touch evenly instead of leaving random gaps.
        let d = s.width * 1.075 / b.closure.sqrt();
        let row_h = d * 0.866;
        let mut shift = Rng::new(mix(g.seed ^ 0x5417, k as i64));
        let (sx, sz) = (shift.f32() * d, shift.f32() * row_h);
        let rows = (
            ((bb[0][1] - sz) / row_h).floor() as i64 - 1,
            ((bb[1][1] - sz) / row_h).ceil() as i64 + 1,
        );
        for j in rows.0..=rows.1 {
            let off = if j.rem_euclid(2) == 1 { 0.5 * d } else { 0.0 };
            let cols = (
                ((bb[0][0] - sx - off) / d).floor() as i64 - 1,
                ((bb[1][0] - sx - off) / d).ceil() as i64 + 1,
            );
            for i in cols.0..=cols.1 {
                visited += 1;
                if visited > 4_000_000 || out.len() >= MAX_PLACEMENTS {
                    break 'species;
                }
                let mut rng = Rng::new(mix(mix(g.seed ^ ((k as u64 + 1) << 40), i), j));
                let p = [
                    sx + off + i as f32 * d + (rng.f32() - 0.5) * 0.5 * d,
                    sz + j as f32 * row_h + (rng.f32() - 0.5) * 0.5 * d,
                ];
                if !point_in_polygon(p, &g.outline) || layout.species_at(p) != Some(k) {
                    continue;
                }
                let (cx, cz) = ((p[0] / cell).floor() as i32, (p[1] / cell).floor() as i32);
                let crowded = (-1..=1).any(|dx| {
                    (-1..=1).any(|dz| {
                        taken.get(&(cx + dx, cz + dz)).is_some_and(|v| {
                            v.iter().any(|(q, w)| {
                                (q[0] - p[0]).hypot(q[1] - p[1]) < 0.3 * 0.5 * (w + s.width)
                            })
                        })
                    })
                });
                if crowded {
                    continue;
                }
                taken.entry((cx, cz)).or_default().push((p, s.width));
                let scale = 0.98 + 0.3 * rng.f32();
                let rotation_y_deg = 360.0 * rng.f32();
                let variant = rng.next() as u32;
                out.push(Placement {
                    asset: s.asset.clone(),
                    position: [p[0], ground(p[0], p[1]).unwrap_or(0.0), p[1]],
                    rotation_y_deg,
                    scale,
                    variant,
                    layer: None,
                });
            }
        }
    }
    // Gaps: where the lattices of two drifts meet, a spot may lie under no crown. A last pass
    // over a fine grid puts a plant of the spot's own drift or matrix on each such spot, so a
    // mature border leaves no bare soil.
    let fine = 0.12f32;
    let (gi0, gi1) = (
        (bb[0][0] / fine).floor() as i64,
        (bb[1][0] / fine).ceil() as i64,
    );
    let (gj0, gj1) = (
        (bb[0][1] / fine).floor() as i64,
        (bb[1][1] / fine).ceil() as i64,
    );
    'fill: for gj in gj0..=gj1 {
        for gi in gi0..=gi1 {
            visited += 1;
            if visited > 4_000_000 || out.len() >= MAX_PLACEMENTS {
                break 'fill;
            }
            let p = [(gi as f32 + 0.5) * fine, (gj as f32 + 0.5) * fine];
            if !point_in_polygon(p, &g.outline) {
                continue;
            }
            let (cx, cz) = ((p[0] / cell).floor() as i32, (p[1] / cell).floor() as i32);
            let covered = (-1..=1).any(|dx| {
                (-1..=1).any(|dz| {
                    taken.get(&(cx + dx, cz + dz)).is_some_and(|v| {
                        v.iter()
                            .any(|(q, w)| (q[0] - p[0]).hypot(q[1] - p[1]) <= 0.65 * w)
                    })
                })
            });
            if covered {
                continue;
            }
            let Some(k) = layout.species_at(p) else {
                continue;
            };
            let s = &infos[k];
            let mut rng = Rng::new(mix(mix(g.seed ^ 0xF177, gi), gj));
            taken.entry((cx, cz)).or_default().push((p, s.width));
            out.push(Placement {
                asset: s.asset.clone(),
                position: [p[0], ground(p[0], p[1]).unwrap_or(0.0), p[1]],
                rotation_y_deg: 360.0 * rng.f32(),
                scale: 0.98 + 0.3 * rng.f32(),
                variant: rng.next() as u32,
                layer: None,
            });
        }
    }
    let truncated = out.len() >= MAX_PLACEMENTS || visited > 4_000_000;
    out.truncate(MAX_PLACEMENTS);
    Evaluation {
        placements: out,
        truncated,
    }
}
