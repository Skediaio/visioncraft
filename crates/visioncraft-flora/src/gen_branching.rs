//! Recursive branching: trees, conifers, multi-stem shrubs, bamboo.
//!
//! The skeleton follows the parametric model of Weber & Penn, "Creation and Rendering of
//! Realistic Trees" (SIGGRAPH 1995): a trunk and up to three levels of children, each level
//! with a count, length ratio, down angle, azimuthal rotation, curvature, and an attraction
//! up/down term (W&P's "AttractionUp", which also produces weeping habits when negative). The
//! length of trunk children follows a crown-shape envelope (W&P's shape functions); deeper
//! children shorten towards the parent's tip as `length * (1 - 0.6 * offset)` (W&P eq. for
//! levels > 1). Phyllotaxis uses the golden angle, opposite/decussate pairs or whorls as in
//! Prusinkiewicz & Lindenmayer, "The Algorithmic Beauty of Plants", ch. 4. As in Runions et al.
//! (2007), the crown envelope bounds the branch system: after growth, the skeleton is scaled so
//! the crown fills the species' stated height and width.
//!
//! The skeleton is LOD independent (its own random stream). Each LOD meshes the thickest
//! branches that fit its wood budget and picks the first N foliage points of one fixed
//! permutation, enlarging cards by sqrt(N0/N) and pulling them inwards so the silhouette and
//! the foliage coverage stay the same.

use crate::math::{GOLDEN_ANGLE, PI, Rng, TAU, UP, V3, cos, deg, sin, v3};
use crate::mesh::{Builder, Card, Node, Outward, rand_dir, shuffled};
use crate::{BranchingParams, Lod, Material, PlantMesh, PlantParams, SeasonState};

struct Branch {
    nodes: Vec<Node>,
    phase: f32,
}

struct LeafPoint {
    p: V3,
    d: V3,
    w: f32,
    phase: f32,
}

#[derive(Default)]
struct Skeleton {
    branches: Vec<Branch>,
    levels: Vec<u8>,
    leaves: Vec<LeafPoint>,
}

struct Ctx<'a> {
    b: &'a BranchingParams,
    p: &'a PlantParams,
    h: f32,
    /// Branch orders a plant of this age has (`skeleton`).
    max_level: usize,
}

/// Wind stiffness per level: how much of the remaining sway a branch of this level adds.
const LEVEL_SWAY: [f32; 4] = [0.25, 0.55, 0.8, 1.0];

fn at(nodes: &[Node], t: f32) -> (V3, f32, f32, V3) {
    let n = nodes.len() - 1;
    let x = (t.clamp(0.0, 1.0) * n as f32).min(n as f32 - 1e-4);
    let i = x.floor() as usize;
    let f = x - i as f32;
    let (a, b) = (nodes[i], nodes[i + 1]);
    (
        a.p.lerp(b.p, f),
        a.r + (b.r - a.r) * f,
        a.w + (b.w - a.w) * f,
        (b.p - a.p).norm(),
    )
}

fn grow(
    sk: &mut Skeleton,
    c: &Ctx,
    level: usize,
    base: V3,
    dir: V3,
    len: f32,
    r0: f32,
    w0: f32,
    phase: f32,
    rng: &mut Rng,
) {
    let b = c.b;
    let (segs, curve, curve_var, gravity) = if level == 0 {
        (
            b.trunk_segments.max(2) as usize,
            0.0,
            b.trunk_wobble_deg,
            0.0,
        )
    } else {
        let l = &b.levels[level - 1];
        (
            l.segments.max(1) as usize,
            l.curve_deg,
            l.curve_var_deg,
            l.gravity,
        )
    };
    let tip = if level == 0 {
        if b.leader < 0.95 { 0.5 } else { 0.08 }
    } else {
        0.15
    };
    let sway = LEVEL_SWAY[level.min(3)];
    let seg_len = len / segs as f32;
    let mut nodes = Vec::with_capacity(segs + 1);
    let (mut p, mut d) = (base, dir.norm());
    for i in 0..=segs {
        let t = i as f32 / segs as f32;
        let mut r = r0 * (1.0 - (1.0 - tip) * t);
        if level == 0 {
            let f = (1.0 - t / 0.1).max(0.0);
            // The flare at the foot comes with age.
            r *= 1.0 + b.trunk_flare * c.p.age * c.p.age * f * f;
        }
        let w = if level == 0 && i == 0 {
            0.0
        } else {
            w0 + (1.0 - w0) * sway * t * t.sqrt()
        };
        nodes.push(Node { p, r, w });
        if i < segs {
            p += d * seg_len;
            let horiz = d.cross(UP);
            let ax = if horiz.len() < 1e-3 {
                d.perp()
            } else {
                horiz.norm()
            };
            // Positive rotation about d x UP turns d towards +Y.
            d = d
                .rotate(ax, deg(curve / segs as f32 + curve_var * rng.sym()))
                .norm();
            let ax2 = d.cross(ax).norm();
            d = d.rotate(ax2, deg(curve_var * 0.7 * rng.sym())).norm();
            d = (d + UP * (gravity / segs as f32)).norm();
        }
    }
    let total_len = len;

    if level < c.max_level && b.levels[level].count > 0 {
        let l = b.levels[level];
        let mut n = l.count as f32;
        if level == 0 {
            // A young tree has fewer limbs.
            n *= 0.5 + 0.5 * c.p.age;
        }
        if level > 0 {
            n *= rng.range(0.8, 1.2);
        }
        let n = (n.round() as usize).max(1);
        let whorl = (l.whorl.max(1)) as usize;
        let groups = n.div_ceil(whorl);
        let mut az = rng.f() * TAU;
        for k in 0..groups {
            let jitter = if whorl > 1 { 0.12 } else { 0.35 };
            let ratio = ((k as f32 + 0.5 + jitter * rng.sym()) / groups as f32).clamp(0.0, 1.0);
            let t = l.start + (1.0 - l.start) * ratio;
            let (pt, r_at, w_at, dp) = at(&nodes, t);
            for j in 0..whorl {
                let mut lc = if level == 0 {
                    c.h * l.length * b.crown_shape.factor(ratio)
                } else {
                    total_len * l.length * (1.0 - 0.6 * t)
                };
                lc *= rng.range(0.85, 1.15);
                let mut down = deg(l.down_angle_deg + l.down_angle_var_deg * rng.sym());
                if level == 0 {
                    down *= 1.0 - 0.45 * ratio; // upper limbs rise more steeply
                }
                let azimuth = az + j as f32 * TAU / whorl as f32 + 0.15 * rng.sym();
                let side = dp.perp().rotate(dp, azimuth);
                let cd = (dp * cos(down) + side * sin(down)).norm();
                let rc = (r_at * l.radius_ratio).min(r_at * 0.9);
                let ph = if level == 0 {
                    rng.f()
                } else {
                    (phase + rng.range(-0.12, 0.12)).rem_euclid(1.0)
                };
                grow(sk, c, level + 1, pt, cd, lc, rc, w_at, ph, rng);
            }
            az += deg(l.rotate_deg) + 0.2 * rng.sym();
        }
    }

    if level >= b.leaf_level as usize {
        let spacing = c.p.leaves.card_m * 0.28;
        let span = total_len * (1.0 - b.leaf_start);
        let count = ((span / spacing).floor() as usize + 1).min(64);
        for i in 0..count {
            let t = if count == 1 {
                1.0
            } else {
                b.leaf_start
                    + (1.0 - b.leaf_start) * (i as f32 + 0.5 * rng.f()) / (count - 1) as f32
            };
            let (lp, _, lw, ld) = at(&nodes, t.min(1.0));
            sk.leaves.push(LeafPoint {
                p: lp,
                d: ld,
                w: lw,
                phase,
            });
        }
    }
    sk.branches.push(Branch { nodes, phase });
    sk.levels.push(level as u8);
}

fn skeleton(p: &PlantParams, b: &BranchingParams, seed: u64) -> Skeleton {
    let mut rng = Rng::new(seed, 1);
    // Age changes the form, not only the size: a young tree is slender (the crown widens later
    // than the tree rises), has a thin trunk and fewer branch orders; its leaves keep their real
    // size, so they are large for the tree. At age 1 nothing changes.
    let h = p.height_m * p.age;
    let w = p.width_m * p.age.powf(1.25);
    let max_level = if p.age < 0.25 {
        1
    } else if p.age < 0.55 {
        2
    } else {
        3
    };
    let c = Ctx { b, p, h, max_level };
    let mut sk = Skeleton::default();
    let stems = b.stems.max(1) as usize;
    for s in 0..stems {
        let (base, dir) = if stems == 1 {
            let lean = deg(b.trunk_wobble_deg * 0.5 * rng.f());
            let az = rng.f() * TAU;
            (
                V3::default(),
                v3(cos(az) * sin(lean), cos(lean), sin(az) * sin(lean)),
            )
        } else {
            let a = s as f32 * GOLDEN_ANGLE + rng.sym() * 0.3;
            let frac = ((s as f32 + 0.5) / stems as f32).sqrt();
            let out = v3(cos(a), 0.0, sin(a));
            let tilt = deg(b.stem_spread_deg) * (0.35 + 0.65 * frac) * rng.range(0.8, 1.2);
            (
                out * (b.stem_base_radius_m * p.age * frac),
                UP * cos(tilt) + out * sin(tilt),
            )
        };
        let len = h * b.leader * if stems > 1 { rng.range(0.75, 1.0) } else { 1.0 };
        let r0 = h * b.trunk_radius_ratio * p.age.powf(0.4) / (stems as f32).sqrt();
        let ph = rng.f();
        grow(&mut sk, &c, 0, base, dir, len, r0, 0.0, ph, &mut rng);
    }
    calibrate(&mut sk, h, w, p.leaves.card_m * 0.5);
    sk
}

/// Scale the skeleton (not the radii or leaf sizes) so the crown fills the stated envelope,
/// and keep hanging branches off the ground.
fn calibrate(sk: &mut Skeleton, h: f32, w: f32, margin: f32) {
    let mut lo = v3(f32::MAX, f32::MAX, f32::MAX);
    let mut hi = v3(f32::MIN, f32::MIN, f32::MIN);
    let mut add = |q: V3| {
        lo = v3(lo.x.min(q.x), lo.y.min(q.y), lo.z.min(q.z));
        hi = v3(hi.x.max(q.x), hi.y.max(q.y), hi.z.max(q.z));
    };
    sk.branches
        .iter()
        .flat_map(|b| b.nodes.iter())
        .for_each(|n| add(n.p));
    sk.leaves.iter().for_each(|l| add(l.p));
    let ext_h = (hi.x - lo.x).max(hi.z - lo.z).max(1e-3);
    let sh = ((w - 2.0 * margin).max(w * 0.3) / ext_h).clamp(0.3, 3.0);
    let sv = ((h - margin).max(h * 0.5) / hi.y.max(1e-3)).clamp(0.4, 2.5);
    let floor = (h * 0.05).min(0.8);
    let scale = |q: V3| v3(q.x * sh, q.y * sv, q.z * sh);
    for (b, &lvl) in sk.branches.iter_mut().zip(&sk.levels) {
        for (i, n) in b.nodes.iter_mut().enumerate() {
            n.p = v3(n.p.x * sh, n.p.y * sv, n.p.z * sh);
            // Hanging branches stop above the ground; only stem bases touch it.
            if (lvl > 0 || i > 0) && n.p.y < floor {
                n.p.y = floor;
            }
        }
    }
    for l in &mut sk.leaves {
        l.p = scale(l.p);
        l.d = v3(l.d.x * sh, l.d.y * sv, l.d.z * sh).norm();
    }
    // Foliage that would end up on the ground (hanging shoots) is dropped rather than piled
    // up along the floor.
    sk.leaves.retain(|l| l.p.y >= floor + margin * 1.2);
}

const SIDES: [[u32; 4]; 3] = [[10, 6, 4, 3], [7, 4, 3, 3], [5, 3, 3, 3]];
const STRIDE: [usize; 3] = [1, 2, 3];
const MIN_R_REL: [f32; 3] = [0.0, 0.05, 0.16];

fn decimate(nodes: &[Node], stride: usize) -> Vec<Node> {
    let mut out: Vec<Node> = nodes.iter().step_by(stride).copied().collect();
    if !(nodes.len() - 1).is_multiple_of(stride) {
        out.push(*nodes.last().unwrap());
    }
    out
}

/// Which branches to mesh at `lod` and what that costs.
fn wood_plan(sk: &Skeleton, lod: usize, wood_budget: usize) -> (Vec<usize>, usize) {
    let trunk_r = sk
        .branches
        .iter()
        .zip(&sk.levels)
        .filter(|(_, l)| **l == 0)
        .map(|(b, _)| b.nodes[0].r)
        .fold(0.0, f32::max);
    let mut order: Vec<usize> = (0..sk.branches.len()).collect();
    // Thickest first (trunk before limbs before twigs); index breaks ties deterministically.
    order.sort_by(|&a, &b| {
        let (la, lb) = (sk.levels[a], sk.levels[b]);
        la.cmp(&lb)
            .then(
                sk.branches[b].nodes[0]
                    .r
                    .total_cmp(&sk.branches[a].nodes[0].r),
            )
            .then(a.cmp(&b))
    });
    let mut used = 0;
    let mut chosen = Vec::new();
    for i in order {
        let lvl = sk.levels[i] as usize;
        let br = &sk.branches[i];
        if lvl > 0 && br.nodes[0].r < trunk_r * MIN_R_REL[lod] {
            continue;
        }
        if lod == 2 && lvl > 1 {
            continue;
        }
        let n = decimate(&br.nodes, STRIDE[lod]).len();
        let cost = Builder::tube_cost(n, SIDES[lod][lvl.min(3)]);
        if used + cost <= wood_budget {
            used += cost;
            chosen.push(i);
        }
    }
    (chosen, used)
}

fn ellipsoid(sk: &Skeleton, h: f32) -> Outward {
    let mut lo = v3(f32::MAX, f32::MAX, f32::MAX);
    let mut hi = v3(f32::MIN, f32::MIN, f32::MIN);
    for l in &sk.leaves {
        lo = v3(lo.x.min(l.p.x), lo.y.min(l.p.y), lo.z.min(l.p.z));
        hi = v3(hi.x.max(l.p.x), hi.y.max(l.p.y), hi.z.max(l.p.z));
    }
    if sk.leaves.is_empty() {
        return Outward::Ellipsoid {
            centre: v3(0.0, h * 0.6, 0.0),
            radii: v3(h * 0.3, h * 0.4, h * 0.3),
        };
    }
    let c = (lo + hi) * 0.5;
    let r = (hi - lo) * 0.5;
    Outward::Ellipsoid {
        centre: c,
        radii: v3(r.x.max(0.2), r.y.max(0.2), r.z.max(0.2)),
    }
}

pub fn generate(
    p: &PlantParams,
    b: &BranchingParams,
    seed: u64,
    lod: Lod,
    st: &SeasonState,
    budget: usize,
) -> PlantMesh {
    let sk = skeleton(p, b, seed);
    let li = lod.index();
    let h = p.height_m * p.age;
    let crown = ellipsoid(&sk, h);
    // Out of leaf, more of the budget goes to wood, the rest to bare-twig cards so the far
    // winter crown still reads as a haze of twigs.
    let wood_share =
        |l: usize| [0.32, 0.28, 0.22][l] + [0.45, 0.25, 0.1][l] * (1.0 - st.leaf_fraction);
    let full_budget = |l: usize| p.budget.triangles(Lod::ALL[l]);
    let (_, wood0) = wood_plan(&sk, 0, (full_budget(0) as f32 * wood_share(0)) as usize);
    let stake = if b.stake_m > 0.0 {
        Builder::tube_cost(2, 4)
    } else {
        0
    };
    let (chosen, wood) = wood_plan(
        &sk,
        li,
        ((budget as f32 * wood_share(li)) as usize).saturating_sub(stake),
    );
    let wood = wood + stake;

    // Foliage card counts: coverage-driven at LOD0, budget-capped at every LOD.
    let card0 = p.leaves.card_m;
    let Outward::Ellipsoid { radii, .. } = crown else {
        unreachable!()
    };
    let area = 4.0 * PI * (radii.x * radii.y + radii.y * radii.z + radii.z * radii.x) / 3.0;
    // `density` 1 means the cards' total area covers the crown envelope's surface 1.5 times.
    let desired = (1.5 * p.leaves.density * area / (card0 * card0)).ceil() as usize;
    let cap = |l: usize, wood: usize| (full_budget(l).saturating_sub(wood)) / 2;
    let npts = sk.leaves.len();
    let n0 = desired.min(cap(0, wood0)).min(npts).max(1);
    // Fewer points than coverage wants: enlarge LOD0 cards to keep the crown closed.
    let c0 = card0 * ((desired as f32 / n0 as f32).sqrt()).clamp(1.0, 2.0);
    let nk = n0.min(cap(li, wood)).max(1);
    let grow_k = (n0 as f32 / nk as f32).sqrt();
    let ck = c0 * grow_k;
    let flowers = p.flowers.as_ref().filter(|_| st.flowers);
    let n_flower = flowers.map_or(0, |f| ((nk as f32 * f.density).round() as usize).min(nk));
    let n_leaf = ((nk as f32 * st.leaf_fraction).round() as usize).min(nk - n_flower.min(nk));
    // Bare twigs as cards are a haze, not a crown: a few of the cards the leaves had (the
    // branches themselves carry the shape); at a third, a winter cherry read as red-brown
    // foliage.
    let n_twig = if p.deciduous {
        ((nk - n_leaf - n_flower) as f32 * 0.12) as usize
    } else {
        0
    };

    let mut out = Builder::default();
    for &i in &chosen {
        let lvl = sk.levels[i] as usize;
        let br = &sk.branches[i];
        out.tube(
            &decimate(&br.nodes, STRIDE[li]),
            SIDES[li][lvl.min(3)],
            br.phase,
        );
    }

    if b.stake_m > 0.0 {
        let r = (0.008 + 0.004 * b.stake_m).min(0.03);
        let x = (h * b.trunk_radius_ratio * 1.5 + r).max(0.03);
        let stake = [
            Node {
                p: v3(x, 0.0, 0.0),
                r,
                w: 0.0,
            },
            Node {
                p: v3(x, b.stake_m * p.age, 0.0),
                r: r * 0.8,
                w: 0.0,
            },
        ];
        out.tube(&stake, 4, 0.0);
    }

    let mut prng = Rng::new(seed, 2);
    let order = shuffled(npts, &mut prng);
    let leaf_cells: [u8; 3] = if grow_k < 1.5 { [0, 1, 2] } else { [4, 5, 6] };
    // Per-point random draws come from a per-point stream so every LOD sees the same card.
    let emit = |out: &mut Builder,
                idx: usize,
                size: f32,
                cell: u8,
                material: Material,
                push_out: f32,
                upness: f32| {
        let lp = &sk.leaves[idx];
        let mut r = Rng::new(seed ^ 0x5eed, 1000 + idx as u64);
        let o = crown.at(lp.p);
        // Hanging shoots (weeping habits) keep their cards hanging; others are randomised.
        let hang = (-lp.d.y - 0.4).clamp(0.0, 0.6) * 4.0;
        let along = (lp.d * (0.45 + hang) + o * 0.3 + rand_dir(&mut r) * 0.8).norm();
        let n = (rand_dir(&mut r) + UP * (upness * 1.2) + o * 0.5).norm();
        let side = along.cross(n).norm();
        let up = n.cross(side).norm();
        let centre = lp.p + o * push_out;
        let card = Card {
            base: centre - up * (size * 0.5),
            up,
            side,
            length: size,
            width: size,
            cell,
            material,
            wind: (lp.w, (lp.w + 0.2).min(1.0)),
            phase: (lp.phase + r.range(0.0, 0.1)).rem_euclid(1.0),
            bend: 0.0,
            segments: 1,
            outward_mix: 0.75,
        };
        out.card(&card, &crown);
    };
    for (k, &idx) in order.iter().take(n_leaf).enumerate() {
        emit(
            &mut out,
            idx,
            ck,
            leaf_cells[k % 3],
            Material::Foliage,
            -(ck - c0) * 0.5,
            // Grown cards (the far levels) lie less flat: a card several leaves wide facing the
            // sky catches the full sun and reads from above as one pale patch on the crown.
            b.leaf_up / grow_k,
        );
    }
    for (k, &idx) in order.iter().skip(n_leaf).take(n_twig).enumerate() {
        emit(
            &mut out,
            idx,
            ck * 0.6,
            12 + (k % 2) as u8,
            Material::Twigs,
            -(ck * 0.6 - c0) * 0.5,
            0.2,
        );
    }
    if let Some(f) = flowers {
        let fk = f.card_m * grow_k.min(2.5);
        for (k, &idx) in order.iter().rev().take(n_flower).enumerate() {
            emit(
                &mut out,
                idx,
                fk,
                8 + (k % 4) as u8,
                Material::Flower,
                f.card_m * 0.3 - (fk - f.card_m) * 0.5,
                0.4,
            );
        }
    }
    debug_assert!(out.tris() <= budget);
    out.finish()
}
