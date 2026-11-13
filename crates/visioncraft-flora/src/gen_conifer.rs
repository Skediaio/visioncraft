//! Excurrent conifers (spruce, fir, larch): a straight leader with whorls of branches whose
//! length falls steadily from the crown base to the tip, so the crown is a cone at every
//! distance. Whorled branching at annual nodes and the excurrent habit follow Prusinkiewicz &
//! Lindenmayer, "The Algorithmic Beauty of Plants" (ch. 4-5) and the tiered branch lengths of
//! Weber & Penn's conical shape function (1995); the sag-then-upturn of spruce branches and the
//! hanging secondary branchlets are standard descriptions of Picea abies habit.

use crate::math::{GOLDEN_ANGLE, PI, Rng, TAU, UP, V3, cos, sin, v3};
use crate::mesh::{Builder, Card, Node, Outward, rand_dir};
use crate::{ConiferParams, Lod, Material, PlantMesh, PlantParams, SeasonState};

struct Pt {
    p: V3,
    tangent: V3,
    out: V3,
    w: f32,
    phase: f32,
    /// Hanging card (pendulous branchlet) rather than a spray.
    hang: bool,
    /// Priority for coarser LODs: lower first (outer branch parts, so the cone edge survives).
    key: f32,
}

pub fn generate(
    p: &PlantParams,
    k: &ConiferParams,
    seed: u64,
    lod: Lod,
    st: &SeasonState,
    budget: usize,
) -> PlantMesh {
    let li = lod.index();
    let h = p.height_m * p.age;
    let w = p.width_m * p.age;
    let c0 = p.leaves.card_m;
    let radius = (w * 0.5 - c0 * 0.45).max(w * 0.15);
    let y_cb = h * k.crown_base;
    let r0 = h * k.trunk_radius_ratio;
    let mut rng = Rng::new(seed, 1);

    // Trunk (leader) to the very top.
    let tsegs = [12usize, 7, 4][li];
    let trunk_r = |y: f32| r0 * (1.0 - 0.92 * (y / h).clamp(0.0, 1.0));
    let trunk_w = |y: f32| 0.3 * (y / h) * (y / h);
    let lean = v3(rng.sym(), 0.0, rng.sym()) * 0.004;
    let trunk_at = |y: f32| v3(lean.x * y, y, lean.z * y);
    let trunk: Vec<Node> = (0..=tsegs)
        .map(|i| {
            let y = h * i as f32 / tsegs as f32;
            let flare = if i == 0 { 1.25 } else { 1.0 };
            Node {
                p: trunk_at(y),
                r: trunk_r(y) * flare,
                w: trunk_w(y),
            }
        })
        .collect();

    // Whorled branches.
    struct Branch {
        nodes: Vec<Node>,
        phase: f32,
    }
    let mut branches = Vec::new();
    let mut pts = Vec::new();
    let tiers = k.tiers.max(2) as usize;
    let per = k.per_whorl.max(1) as usize;
    let spacing = c0 * 0.22;
    for i in 0..tiers {
        let f = ((i as f32 + 0.5 + 0.25 * rng.sym()) / tiers as f32).clamp(0.0, 1.0);
        let y = y_cb + f * (h * 0.96 - y_cb);
        let base_len = radius * pow(1.0 - f, k.taper).max(0.04);
        let az0 = i as f32 * GOLDEN_ANGLE;
        for j in 0..per {
            let az = az0 + TAU * j as f32 / per as f32 + 0.35 * rng.sym();
            let len = base_len * rng.range(0.85, 1.1);
            let out = v3(cos(az), 0.0, sin(az));
            let phase = rng.f();
            let start = trunk_at(y);
            let w0 = trunk_w(y);
            let segs = 4;
            let mut nodes = Vec::with_capacity(segs + 1);
            for s in 0..=segs {
                let t = s as f32 / segs as f32;
                // Sag in the middle, tip swept up (spruce); fir nearly flat.
                let dy = len
                    * (-k.droop * 0.35 * sin(PI * t) * (1.0 - 0.3 * t)
                        + k.upsweep * 0.22 * t * t * t
                        - k.droop * 0.12 * t);
                let q = start + out * (len * t) + UP * dy;
                nodes.push(Node {
                    p: q,
                    r: (trunk_r(y) * 0.45).max(0.004) * (1.0 - 0.85 * t),
                    w: (w0 + (1.0 - w0) * 0.8 * t * t.sqrt()).min(1.0),
                });
            }
            // Foliage points along the outer 85 % of the branch (inner shaded parts are bare).
            // A branch and its laterals form a flat fan that widens outwards; points spread
            // over that fan, so the crown stays closed at the base where branches diverge.
            let fan = (TAU * len / per as f32 * 0.5).min(len * 0.7);
            let n = (((len * 0.85 / spacing) * (1.0 + fan / (2.0 * c0))) as usize + 1).min(96);
            let lateral = v3(-out.z, 0.0, out.x);
            for m in 0..n {
                let t = 0.15 + 0.85 * (m as f32 + rng.f() * 0.6) / n as f32;
                let t = t.min(1.0);
                let x = t * segs as f32;
                let a = (x.floor() as usize).min(segs - 1);
                let fr = x - a as f32;
                let q = nodes[a].p.lerp(nodes[a + 1].p, fr) + lateral * (rng.sym() * fan * t * 0.5);
                let tangent = (nodes[a + 1].p - nodes[a].p).norm();
                let wv = nodes[a].w + (nodes[a + 1].w - nodes[a].w) * fr;
                let hang = rng.f() < k.pendulous * 0.5;
                let key = rng.f() * 0.55 + (1.0 - t) * 0.45;
                pts.push(Pt {
                    p: q,
                    tangent,
                    out,
                    w: wv,
                    phase,
                    hang,
                    key,
                });
            }
            branches.push(Branch { nodes, phase });
        }
    }
    // The leader's top: a few points around the spire so the tip is green.
    let spire = (c0 * 2.0).min(h * 0.15);
    for m in 0..6 {
        let y = h - spire * (m as f32 / 6.0) - c0 * 0.4;
        let az = m as f32 * GOLDEN_ANGLE;
        pts.push(Pt {
            p: trunk_at(y.max(0.0)),
            tangent: UP,
            out: v3(cos(az), 0.0, sin(az)),
            w: trunk_w(y) + 0.2,
            phase: 0.0,
            hang: false,
            key: 0.1,
        });
    }

    let mut out = Builder::default();
    out.tube(&trunk, [10, 7, 5][li], 0.0);
    // Branch wood: lowest (thickest, longest) first within the LOD's wood share.
    let leafless = 1.0 - st.leaf_fraction;
    let wood_share = [0.22, 0.16, 0.08][li] + [0.25, 0.15, 0.06][li] * leafless;
    let wood_budget = (budget as f32 * wood_share) as usize;
    let stride = [1usize, 2, 4][li];
    for b in &branches {
        // Four segments: every stride (1, 2, 4) keeps both ends.
        let nodes: Vec<Node> = b.nodes.iter().step_by(stride).copied().collect();
        let cost = Builder::tube_cost(nodes.len(), 3);
        if out.tris() + cost > wood_budget || nodes[0].r < r0 * [0.0, 0.03, 0.08][li] {
            continue;
        }
        out.tube(&nodes, 3, b.phase);
    }
    let wood = out.tris();

    // Cards: coverage from the cone's lateral area, capped by budget; one permutation for all LODs.
    let slant = ((h - y_cb) * (h - y_cb) + radius * radius).sqrt();
    let area = PI * radius * slant;
    let desired = (p.leaves.density * 1.6 * area / (c0 * c0)) as usize;
    let wood0 =
        trunk_cost(0) + (p.budget.triangles(Lod::Lod0) as f32 * (0.22 + 0.25 * leafless)) as usize;
    let cap0 = p
        .budget
        .triangles(Lod::Lod0)
        .saturating_sub(wood0.min(p.budget.triangles(Lod::Lod0)))
        / 2;
    let n0 = desired.min(cap0).min(pts.len()).max(1);
    let ck_cap = budget.saturating_sub(wood) / 2;
    let nk = n0.min(ck_cap).max(1);
    let grow = (n0 as f32 / nk as f32).sqrt();
    let ck = c0 * grow;
    let n_leaf = ((nk as f32 * st.leaf_fraction).round() as usize).min(nk);
    let n_twig = if p.deciduous {
        ((nk - n_leaf) as f32 * 0.8) as usize
    } else {
        0
    };
    let cells: [u8; 3] = if grow < 1.5 { [0, 1, 2] } else { [4, 5, 6] };
    let field = Outward::Axis {
        centre: V3::default(),
        up: (radius / (h - y_cb).max(0.1)).clamp(0.2, 0.8),
    };
    // One fixed priority order for every LOD: LOD k uses the first N_k points.
    let mut order: Vec<usize> = (0..pts.len()).collect();
    order.sort_by(|&a, &b| pts[a].key.total_cmp(&pts[b].key).then(a.cmp(&b)));
    let emit = |out: &mut Builder, idx: usize, cell: u8, material: Material| {
        let pt = &pts[idx];
        let mut r = Rng::new(seed ^ 0xC0FE, 300 + idx as u64);
        let jitter = rand_dir(&mut r);
        let size = ck * r.range(0.85, 1.15);
        let (up, normal, base) = if pt.hang && material == Material::Foliage {
            // A hanging branchlet: card plane faces outward, hangs from the branch.
            let up = (-UP + jitter * 0.25).norm();
            (
                up,
                (pt.out + jitter * 0.3).norm(),
                pt.p - pt.out * ((size - c0) * 0.5),
            )
        } else {
            // Sprays follow the branch; their faces are random with an upward bias (more so
            // for layered firs), so the crown edge never shows cards edge-on.
            let along = (pt.tangent + jitter * 0.35).norm();
            let normal = (jitter + UP * (0.35 + 0.9 * k.layered) + pt.out * 0.25).norm();
            let centre = pt.p - pt.out * ((size - c0) * 0.5);
            (along, normal, centre - along * (size * 0.5))
        };
        let side = up.cross(normal).norm();
        let up = normal.cross(side).norm();
        out.card(
            &Card {
                base,
                up,
                side,
                length: size,
                width: size,
                cell,
                material,
                wind: (pt.w, (pt.w + 0.15).min(1.0)),
                phase: (pt.phase + r.range(0.0, 0.1)).rem_euclid(1.0),
                bend: 0.0,
                segments: 1,
                outward_mix: 0.7,
            },
            &field,
        );
    };
    if li < 2 {
        for (m, &idx) in order.iter().take(n_leaf).enumerate() {
            emit(&mut out, idx, cells[m % 3], Material::Foliage);
        }
        for (m, &idx) in order.iter().skip(n_leaf).take(n_twig).enumerate() {
            emit(&mut out, idx, 12 + (m % 2) as u8, Material::Twigs);
        }
    } else {
        // Far LOD: a few large cards per tier, sized to that tier's branch length, so the
        // stack of tiers draws the cone outline directly.
        let n_cards = if n_leaf > 0 { n_leaf } else { n_twig };
        let groups = (n_cards / 9).clamp(3, 18);
        let per = (n_cards / groups).max(1);
        let mut r = Rng::new(seed ^ 0x51DE, 9);
        for g in 0..if n_cards > 0 { groups } else { 0 } {
            let f = (g as f32 + 0.5) / groups as f32;
            let y = y_cb + f * (h * 0.96 - y_cb);
            let reach = radius * pow(1.0 - f, k.taper).max(0.05);
            let tier_h = (h - y_cb) / groups as f32;
            for m in 0..per {
                let az = TAU * (m as f32 + 0.5 * r.f()) / per as f32 + g as f32 * GOLDEN_ANGLE;
                let outd = v3(cos(az), 0.0, sin(az));
                let size = reach * 1.15 + tier_h * 0.6;
                let droop_v = -k.droop * 0.3 + 0.1;
                let along = (outd + UP * droop_v + rand_dir(&mut r) * 0.15).norm();
                let normal = (rand_dir(&mut r) * 0.8 + UP * (0.5 + 0.8 * k.layered)).norm();
                let side = along.cross(normal).norm();
                let base = trunk_at(y) + outd * (reach * 0.5 - size * 0.5) + UP * (tier_h * 0.3);
                let (material, cell) = if n_leaf > 0 {
                    (Material::Foliage, 4 + (m % 3) as u8)
                } else {
                    (Material::Twigs, 12 + (m % 2) as u8)
                };
                let wv = (trunk_w(y) + 0.3).min(1.0);
                out.card(
                    &Card {
                        base,
                        up: along,
                        side,
                        length: size,
                        width: size * 0.7,
                        cell,
                        material,
                        wind: (trunk_w(y), wv),
                        phase: r.f(),
                        bend: 0.0,
                        segments: 1,
                        outward_mix: 0.7,
                    },
                    &field,
                );
            }
        }
    }
    debug_assert!(out.tris() <= budget, "{} > {budget}", out.tris());
    out.finish()
}

fn trunk_cost(li: usize) -> usize {
    Builder::tube_cost([12usize, 7, 4][li] + 1, [10, 7, 5][li])
}

/// `x^e` for x in [0, 1] via exp/log-free bisection on the exponent's binary expansion; exact
/// enough for shape exponents and deterministic on every platform.
fn pow(x: f32, e: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut result = 1.0f32;
    let mut whole = e.floor();
    while whole >= 1.0 {
        result *= x;
        whole -= 1.0;
    }
    let mut frac = e - e.floor();
    let mut root = x;
    for _ in 0..12 {
        root = root.sqrt();
        frac *= 2.0;
        if frac >= 1.0 {
            result *= root;
            frac -= 1.0;
        }
    }
    result
}
