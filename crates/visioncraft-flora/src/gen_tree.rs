//! Tree skeletons grown by space colonisation, with twigs and leaf clusters down to the
//! current year's shoots, and the leaf-level meshes drawn near the camera on strong GPUs.
//!
//! * Crown structure: A. Runions, B. Lane & P. Prusinkiewicz, "Modeling Trees with a Space
//!   Colonization Algorithm", Eurographics Workshop on Natural Phenomena 2007, pp. 63-70.
//!   Attraction points fill the species' crown envelope; each point pulls the nearest tree node
//!   within the radius of influence, a node grows one step of length D towards the mean
//!   direction of its points, and points within the kill distance of a node are removed.
//!   Lateral growth from older nodes gives the branching; tropism (gravitropism, a leader's
//!   rise, a weeping habit's droop) is added to the growth direction as in section 4 there.
//! * Light: the shadow propagation of W. Palubicki et al., "Self-organizing Tree Models for
//!   Image Synthesis", ACM TOG 28(3) 58 (SIGGRAPH 2009), section 4.1: every shoot shades a
//!   pyramid of voxels below it, falling off geometrically with depth. Shoots in deep shade
//!   carry no leaves (they are shed, as on a real tree), so the foliage forms the crown's shell.
//! * Radii: the pipe model of K. Shinozaki, K. Yoda, K. Hozumi & T. Kira, "A quantitative
//!   analysis of plant form - the pipe model theory", Japanese Journal of Ecology 14 (1964),
//!   pp. 97-105 and 133-139: the cross-section of a branch is proportional to the leaves it
//!   carries, so at a fork the parent's area is the sum of its children's (exponent 2).
//! * Twigs and shoots: phyllotaxis (alternate spiral, distichous, opposite-decussate) after
//!   P. Prusinkiewicz & A. Lindenmayer, "The Algorithmic Beauty of Plants" (1990), ch. 4.
//!
//! Everything is a pure function of the parameters and the seed. Lengths are absolute (the
//! mature crown sets the spacing), so a young tree of a species has fewer branch orders than
//! the old one, and its leaves keep their real size.

use crate::gen_branching::{Branch, LeafPoint, Shoot, Skeleton};
use crate::math::{GOLDEN_ANGLE, PI, Rng, TAU, UP, V3, cos, deg, sin, v3};
use crate::mesh::{Builder, Card, Node, Outward};
use crate::{BranchingParams, BudgetClass, Material, PlantMesh, PlantParams, SeasonState};

/// The species' crown envelope: radius (relative to the half width) at `t` = 0 at the crown
/// base to 1 at the top, the Weber & Penn shape closed at both ends.
fn envelope(b: &BranchingParams, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let s = 2.0 * t - 1.0;
    let s3 = s * s * s;
    let close = (1.0 - s3 * s3).max(0.0).sqrt();
    let peak = (0..=16)
        .map(|k| b.crown_shape.factor(k as f32 / 16.0))
        .fold(0.0f32, f32::max)
        .max(1e-3);
    b.crown_shape.factor(t) / peak * close
}

/// A uniform grid of buckets for neighbour queries.
struct Grid {
    lo: V3,
    cell: f32,
    n: [i32; 3],
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(lo: V3, hi: V3, cell: f32) -> Grid {
        let dim = |a: f32, b: f32| (((b - a) / cell).ceil() as i32 + 1).clamp(1, 256);
        let n = [dim(lo.x, hi.x), dim(lo.y, hi.y), dim(lo.z, hi.z)];
        Grid {
            lo,
            cell,
            n,
            cells: vec![Vec::new(); (n[0] * n[1] * n[2]) as usize],
        }
    }
    fn key(&self, p: V3) -> [i32; 3] {
        let k = |v: f32, lo: f32, n: i32| (((v - lo) / self.cell).floor() as i32).clamp(0, n - 1);
        [
            k(p.x, self.lo.x, self.n[0]),
            k(p.y, self.lo.y, self.n[1]),
            k(p.z, self.lo.z, self.n[2]),
        ]
    }
    fn index(&self, k: [i32; 3]) -> usize {
        ((k[1] * self.n[2] + k[2]) * self.n[0] + k[0]) as usize
    }
    fn insert(&mut self, p: V3, id: u32) {
        let i = self.index(self.key(p));
        self.cells[i].push(id);
    }
    /// Every id in the 3x3x3 cells around `p`.
    fn around(&self, p: V3, mut f: impl FnMut(u32)) {
        let k = self.key(p);
        for y in (k[1] - 1).max(0)..=(k[1] + 1).min(self.n[1] - 1) {
            for z in (k[2] - 1).max(0)..=(k[2] + 1).min(self.n[2] - 1) {
                for x in (k[0] - 1).max(0)..=(k[0] + 1).min(self.n[0] - 1) {
                    for &id in &self.cells[self.index([x, y, z])] {
                        f(id);
                    }
                }
            }
        }
    }
}

/// The space-colonisation tree: nodes in creation order (a parent always before its children).
#[derive(Default)]
struct Sc {
    p: Vec<V3>,
    d: Vec<V3>,
    parent: Vec<u32>,
    kids: Vec<u8>,
    order: Vec<u8>,
}

const NONE: u32 = u32::MAX;

impl Sc {
    fn push(&mut self, p: V3, d: V3, parent: u32, order: u8) -> u32 {
        self.p.push(p);
        self.d.push(d);
        self.parent.push(parent);
        self.kids.push(0);
        self.order.push(order);
        if parent != NONE {
            self.kids[parent as usize] += 1;
        }
        (self.p.len() - 1) as u32
    }
}

/// The spacing of the branch network for a species: the kill distance `d_k`; the step is
/// about a third of it and the radius of influence four times it. Set by the
/// mature crown so every age of a species shares it.
fn kill_distance(p: &PlantParams) -> f32 {
    (0.03 * p.width_m.max(p.height_m * 0.8)).clamp(0.15, 0.8)
}

/// Rotate `g` away from `axis` until it makes at least `min` radians with it.
fn at_least(g: V3, axis: V3, min: f32) -> V3 {
    let c = g.dot(axis).clamp(-1.0, 1.0);
    if c <= cos(min) {
        return g;
    }
    let side = (g - axis * c).len();
    let perp = if side > 1e-4 {
        (g - axis * c) * (1.0 / side)
    } else {
        axis.perp()
    };
    (axis * cos(min) + perp * sin(min)).norm()
}

/// Leaf-cluster card edge, metres: a cluster cell (atlas cells 14 and 15) draws a shoot with
/// its leaves at their true size on a card this large.
pub(crate) fn cluster_m(p: &PlantParams) -> f32 {
    p.leaves.leaf_m * 2.6
}

/// A shoot's length: the twig drawn in the cluster cell.
fn shoot_m(p: &PlantParams) -> f32 {
    cluster_m(p) * 0.52
}

/// Grows the skeleton of a tree: trunk, limbs and branches by space colonisation inside the
/// species' envelope, then twigs and the current year's shoots by phyllotaxis, each shoot with
/// the light it gets. `h` and `w` are the aged height and crown width.
pub(crate) fn skeleton(
    p: &PlantParams,
    b: &BranchingParams,
    seed: u64,
    h: f32,
    w: f32,
) -> Skeleton {
    let mut rng = Rng::new(seed, 11);
    let dk = kill_distance(p);
    let step = dk * 0.32;
    let di = dk * 4.0;
    let y0 = (b.crown_base * h).min(h * 0.6);
    let half = w * 0.5;

    // Attraction points in the envelope. A real crown is no smooth dome: its limbs' branch
    // systems bulge out and leave bays between them, more so for tortuous species (oak) than
    // for straight ones (beech). Two sums of random plane waves per seed bend the envelope and
    // thin the points between the lumps, so crowns of one species differ.
    let waves = |rng: &mut Rng| -> [(V3, f32); 4] {
        std::array::from_fn(|_| {
            (
                crate::mesh::rand_dir(rng) * rng.range(2.2, 3.6),
                rng.f() * TAU,
            )
        })
    };
    let (bulge, lumps) = (waves(&mut rng), waves(&mut rng));
    let rough = (b.levels[0].curve_var_deg / 40.0).clamp(0.15, 0.5);
    let centre = v3(0.0, (y0 + h) * 0.5, 0.0);
    let scale = 1.0 / half.max((h - y0) * 0.5).max(0.1);
    let noise = |w: &[(V3, f32); 4], q: V3| -> f32 {
        let u = (q - centre) * scale;
        w.iter().map(|(k, ph)| sin(u.dot(*k) + ph)).sum::<f32>() * 0.4
    };
    let inside = |q: V3| -> bool {
        let t = (q.y - y0) / (h - y0).max(1e-3);
        if !(0.0..=1.0).contains(&t) {
            return false;
        }
        let r = half * envelope(b, t) * (1.0 + rough * noise(&bulge, q)).max(0.3);
        q.x * q.x + q.z * q.z <= r * r
    };
    let vol = PI * half * half * (h - y0) * 0.6;
    let spacing = dk * 1.15;
    let n_points = ((vol / (spacing * spacing * spacing)) as usize).clamp(60, 7000);
    let lo = v3(-half * 1.3, 0.0, -half * 1.3);
    let hi = v3(half * 1.3, h * 1.1, half * 1.3);
    let mut pts: Vec<V3> = Vec::with_capacity(n_points);
    let mut tries = 0;
    while pts.len() < n_points && tries < n_points * 40 {
        tries += 1;
        let q = v3(
            rng.sym() * half * 1.15,
            y0 + rng.f() * (h - y0),
            rng.sym() * half * 1.15,
        );
        // Never empty: a gap wider than the radius of influence stops growth across it.
        let keep = 0.6 + 0.4 * noise(&lumps, q).clamp(-1.0, 1.0);
        if inside(q) && rng.f() < keep {
            pts.push(q);
        }
    }
    let mut pgrid = Grid::new(lo, hi, di);
    for (i, q) in pts.iter().enumerate() {
        pgrid.insert(*q, i as u32);
    }
    let mut alive = vec![true; pts.len()];
    let mut near = vec![NONE; pts.len()];
    let mut near_d = vec![f32::MAX; pts.len()];

    // Stems: straight-ish up to just below the crown, as the old generator set them.
    let mut sc = Sc::default();
    let stems = b.stems.max(1) as usize;
    let mut srng = Rng::new(seed, 12);
    let below = (y0 - di * 0.4).max(step);
    for s in 0..stems {
        let (base, mut dir) = if stems == 1 {
            let lean = deg(b.trunk_wobble_deg * 0.5 * srng.f());
            let az = srng.f() * TAU;
            (
                V3::default(),
                v3(cos(az) * sin(lean), cos(lean), sin(az) * sin(lean)),
            )
        } else {
            let a = s as f32 * GOLDEN_ANGLE + srng.sym() * 0.3;
            let frac = ((s as f32 + 0.5) / stems as f32).sqrt();
            let out = v3(cos(a), 0.0, sin(a));
            let tilt = deg(b.stem_spread_deg) * (0.35 + 0.65 * frac) * srng.range(0.8, 1.2);
            (
                out * (b.stem_base_radius_m * p.age * frac),
                UP * cos(tilt) + out * sin(tilt),
            )
        };
        let mut i = sc.push(base, dir, NONE, 0);
        while sc.p[i as usize].y < below {
            let wob = deg(b.trunk_wobble_deg * 0.25);
            dir = (dir.rotate(dir.perp(), wob * srng.sym()) + UP * 0.15).norm();
            let q = sc.p[i as usize] + dir * step;
            i = sc.push(q, dir, i, 0);
        }
    }

    // Each point's nearest node within the radius of influence; points within the kill
    // distance of a node die. Only new nodes can change either, so they are checked alone.
    let mut changed: Vec<u32> = (0..sc.p.len() as u32).collect();
    let max_kids = |order: u8| 1 + b.levels[(order as usize).min(2)].whorl.clamp(1, 3);
    let lvl = |order: u8| &b.levels[(order as usize).saturating_sub(1).min(2)];
    let mut infl: Vec<V3> = Vec::new();
    let mut cnt: Vec<u16> = Vec::new();
    for _iter in 0..600 {
        for &n in &changed {
            let q = sc.p[n as usize];
            pgrid.around(q, |pi| {
                let pi = pi as usize;
                if !alive[pi] {
                    return;
                }
                let d = (pts[pi] - q).len();
                if d < dk {
                    alive[pi] = false;
                } else if d < di && d < near_d[pi] {
                    near_d[pi] = d;
                    near[pi] = n;
                }
            });
        }
        infl.clear();
        infl.resize(sc.p.len(), V3::default());
        cnt.clear();
        cnt.resize(sc.p.len(), 0);
        let mut any = false;
        for pi in 0..pts.len() {
            if alive[pi] && near[pi] != NONE {
                let n = near[pi] as usize;
                infl[n] += (pts[pi] - sc.p[n]).norm();
                cnt[n] += 1;
                any = true;
            }
        }
        if !any {
            break;
        }
        changed.clear();
        let count = sc.p.len();
        for n in 0..count {
            if cnt[n] == 0 || sc.kids[n] >= max_kids(sc.order[n]) {
                continue;
            }
            let pull = infl[n];
            if pull.len() < 1e-3 {
                continue;
            }
            let d0 = sc.d[n];
            let tip = sc.kids[n] == 0;
            let order = if tip { sc.order[n] } else { sc.order[n] + 1 };
            let l = lvl(order.max(1));
            // Tropism: a leader rises up to `leader` of the height, branches follow their level's
            // gravity (weeping habits droop), every growth step bends a little at random
            // (tortuous oak, straight beech).
            let rise = if order == 0 && sc.p[n].y < b.leader * h {
                0.35 + 0.5 * b.leader
            } else {
                0.0
            };
            let trop = UP * (rise + 0.12 * l.gravity);
            let mut g = pull.norm() + trop;
            if tip {
                // Tortuous species hold their course less (an oak's zigzag).
                g = g + d0 * (0.8 * (1.0 - rough));
            }
            let mut g = g.norm();
            if !tip {
                // A lateral leaves its parent at the species' branching angle at least.
                g = at_least(g, d0, deg(l.down_angle_deg * 0.6));
            }
            let wob = deg(l.curve_var_deg * (0.3 + rough));
            g = g.rotate(g.perp(), wob * rng.sym()).norm();
            let q = sc.p[n] + g * step;
            let id = sc.push(q, g, n as u32, order);
            changed.push(id);
        }
        // Points two nodes pull at from opposite sides can keep a pair growing past each other
        // (a known case of the algorithm); a crown never needs this many nodes.
        if changed.is_empty() || sc.p.len() > 30_000 {
            break;
        }
    }
    finish(p, b, seed, sc, h, y0, dk, step)
}

/// From the colonised network to the shared skeleton: twigs and shoots, light, pipe-model
/// radii, wind weights, branch chains and leaf points.
fn finish(
    p: &PlantParams,
    b: &BranchingParams,
    seed: u64,
    sc: Sc,
    h: f32,
    y0: f32,
    dk: f32,
    step: f32,
) -> Skeleton {
    let n = sc.p.len();
    let mut kids: Vec<Vec<u32>> = vec![Vec::new(); n];
    for i in 1..n {
        if sc.parent[i] != NONE {
            kids[sc.parent[i] as usize].push(i as u32);
        }
    }
    // Tips downstream of each node (the periphery is where twigs grow).
    let mut tips = vec![0u32; n];
    for i in (0..n).rev() {
        if kids[i].is_empty() {
            tips[i] += 1;
        }
        if sc.parent[i] != NONE {
            tips[sc.parent[i] as usize] += tips[i];
        }
    }
    // Path length from the ground: the wind's bend weight grows along it.
    let mut dist = vec![0.0f32; n];
    for i in 0..n {
        if sc.parent[i] != NONE {
            let q = sc.parent[i] as usize;
            dist[i] = dist[q] + (sc.p[i] - sc.p[q]).len();
        }
    }
    let far = dist.iter().fold(1e-3f32, |a, &d| a.max(d)) + shoot_m(p) * 2.0;
    let bend = |d: f32| {
        let t = (d / far).clamp(0.0, 1.0);
        t * t.sqrt()
    };
    // A sway phase per limb, inherited along it with a little drift.
    let mut prng = Rng::new(seed, 13);
    let mut phase = vec![0.0f32; n];
    for i in 0..n {
        phase[i] = if sc.parent[i] == NONE {
            prng.f()
        } else if sc.order[i] != sc.order[sc.parent[i] as usize] {
            (phase[sc.parent[i] as usize] + prng.range(-0.3, 0.3)).rem_euclid(1.0)
        } else {
            phase[sc.parent[i] as usize]
        };
    }

    // Twigs (one order below the colonised network) and shoots on them, by phyllotaxis, from
    // the outer nodes. Each twig is a short curved stick of three nodes.
    let mut trng = Rng::new(seed, 14);
    let ls = shoot_m(p);
    // Weeping species (birch) hang long thin strands.
    let twig_l = (step * 1.6).max(ls * 2.2) * (1.0 + 0.6 * (-b.levels[1].gravity).max(0.0));
    let l2 = &b.levels[1.min(2)];
    let l3 = &b.levels[2];
    let floor = (h * 0.05).min(0.8);
    let periph = 6;
    let mut twigs: Vec<(u32, [V3; 3], f32)> = Vec::new(); // (owner node, points, phase)
    // (owner node, owner twig or NONE, shoot)
    let mut shoots: Vec<(u32, u32, Shoot)> = Vec::new();
    let mut add_shoot = |owner: (u32, u32), at: V3, dir: V3, wgt: f32, ph: f32, rng: &mut Rng| {
        let mut d = dir;
        // Pendulous shoots (birch) hang from their twig.
        d = (d + UP * (0.25 * l3.gravity)).norm();
        let len = ls * rng.range(0.8, 1.2);
        if at.y + d.y * len < floor {
            return;
        }
        shoots.push((
            owner.0,
            owner.1,
            Shoot {
                p: at,
                d,
                len,
                w: (wgt + 0.1).min(1.0),
                phase: (ph + rng.range(-0.08, 0.08)).rem_euclid(1.0),
                light: 0.0,
            },
        ));
    };
    let div_twig = deg(l2.rotate_deg);
    let div_shoot = deg(l3.rotate_deg);
    let whorl3 = l3.whorl.clamp(1, 3) as usize;
    for i in 0..n {
        if tips[i] > periph || sc.p[i].y < y0 * 0.8 {
            continue;
        }
        let d = sc.d[i];
        let wgt = bend(dist[i]);
        let ph = phase[i];
        // The node's own terminal shoot where the network ends.
        if kids[i].is_empty() {
            add_shoot((i as u32, NONE), sc.p[i], d, wgt, ph, &mut trng);
        }
        let mut az = trng.f() * TAU;
        // Twigs about every 0.8 shoot lengths along the branch, more at its end.
        let along = ((step / (ls * 0.8)).ceil() as usize).max(1);
        let ntw = if kids[i].is_empty() { along + 1 } else { along };
        for _ in 0..ntw {
            az += div_twig;
            let side = d.perp().rotate(d, az);
            let a = deg(l2.down_angle_deg + l2.down_angle_var_deg * trng.sym());
            let td = (d * cos(a) + side * sin(a) + UP * (0.15 * l2.gravity)).norm();
            let len = twig_l * trng.range(0.7, 1.15);
            // Spread along the parent's last step, not all from its node.
            let q0 = sc.p[i] - d * (step * trng.f() * 0.9);
            let q1 = q0 + td * (len * 0.5);
            let q2 = q0 + td * len;
            if q2.y < floor {
                continue;
            }
            let tph = (ph + trng.range(-0.1, 0.1)).rem_euclid(1.0);
            let tw = twigs.len() as u32;
            twigs.push((i as u32, [q0, q1, q2], tph));
            // Shoots along the twig at the species' divergence, a terminal one at its end.
            let k = ((len / (ls * 0.6)) as usize).clamp(1, 6);
            let mut saz = trng.f() * TAU;
            for j in 0..k {
                let t = 0.25 + 0.7 * (j as f32 + 0.5) / k as f32;
                let at = if t < 0.5 {
                    q0.lerp(q1, t * 2.0)
                } else {
                    q1.lerp(q2, t * 2.0 - 1.0)
                };
                let axis = td;
                for m in 0..whorl3 {
                    let s = axis
                        .perp()
                        .rotate(axis, saz + m as f32 * TAU / whorl3 as f32);
                    let a = deg(l3.down_angle_deg + l3.down_angle_var_deg * trng.sym());
                    let sd = (axis * cos(a) + s * sin(a)).norm();
                    add_shoot((i as u32, tw), at, sd, (wgt + 0.1).min(1.0), tph, &mut trng);
                }
                saz += div_shoot;
            }
            add_shoot(
                (i as u32, tw),
                q2,
                td,
                (wgt + 0.15).min(1.0),
                tph,
                &mut trng,
            );
        }
    }

    // Light (Palubicki et al. 2009, 4.1): each shoot shades the voxels below it in a pyramid,
    // a * b^-q at depth q.
    let vox = (dk * 0.55).max(ls * 1.2);
    let (mut lo, mut hi) = (
        v3(f32::MAX, f32::MAX, f32::MAX),
        v3(f32::MIN, f32::MIN, f32::MIN),
    );
    for (_, _, s) in &shoots {
        lo = v3(lo.x.min(s.p.x), lo.y.min(s.p.y), lo.z.min(s.p.z));
        hi = v3(hi.x.max(s.p.x), hi.y.max(s.p.y), hi.z.max(s.p.z));
    }
    if !shoots.is_empty() {
        let g = Grid::new(lo, hi, vox);
        let [nx, ny, nz] = g.n;
        let mut shade = vec![0.0f32; (nx * ny * nz) as usize];
        // A voxel's shadow saturates with the shoots in it; spread over the pyramid's layer so a
        // dense layer above dims a voxel by about `a / B^q` (a Beer-Lambert reading of it below).
        const B: f32 = 1.5;
        const QMAX: i32 = 7;
        // Shoots in one voxel shade together: count them first.
        let mut count = vec![0u16; shade.len()];
        for (_, _, s) in &shoots {
            let i = g.index(g.key(s.p));
            count[i] = count[i].saturating_add(1);
        }
        for y in 0..ny {
            for z in 0..nz {
                for x in 0..nx {
                    let c = count[g.index([x, y, z])] as f32;
                    if c == 0.0 {
                        continue;
                    }
                    let mut a = c.min(4.0) * 0.5;
                    for q in 1..=QMAX {
                        a /= B;
                        let yy = y - q;
                        if yy < 0 {
                            break;
                        }
                        let per = a / ((2 * q + 1) * (2 * q + 1)) as f32;
                        for zz in (z - q).max(0)..=(z + q).min(nz - 1) {
                            for xx in (x - q).max(0)..=(x + q).min(nx - 1) {
                                shade[g.index([xx, yy, zz])] += per;
                            }
                        }
                    }
                }
            }
        }
        // Bays: a crown's outline is broken by gaps where a limb's branch system ends and the
        // next begins, deeper in tortuous species. Shoots in them get little light.
        let mut brng = Rng::new(seed, 15);
        let bays: [(V3, f32); 3] = std::array::from_fn(|_| {
            (
                crate::mesh::rand_dir(&mut brng) * brng.range(5.0, 8.0),
                brng.f() * TAU,
            )
        });
        let rough = (b.levels[0].curve_var_deg / 40.0).clamp(0.15, 0.5);
        let (c, inv) = ((lo + hi) * 0.5, 1.0 / ((hi - lo).len() * 0.5).max(0.1));
        for (_, _, s) in &mut shoots {
            let u = (s.p - c) * inv;
            let bay: f32 = bays.iter().map(|(k, ph)| sin(u.dot(*k) + ph)).sum::<f32>() / 3.0;
            if bay < rough - 0.95 {
                s.light = 0.0;
                continue;
            }
            let i = g.index(g.key(s.p));
            // About exp(-2.4 s), in plain arithmetic so it is the same on every platform.
            let k = 1.0 + 1.2 * shade[i];
            s.light = 1.0 / (k * k);
        }
    }

    // Pipe model: a branch's cross-section carries the shoots beyond it (Shinozaki et al. 1964).
    let mut area = vec![0.0f32; n];
    let mut twig_area = vec![0.0f32; twigs.len()];
    for (owner, twig, s) in &shoots {
        // Shed (deep-shade) shoots still had their wood once: half weight.
        let a = 0.5 + 0.5 * s.light;
        if *twig != NONE {
            twig_area[*twig as usize] += a;
        } else {
            area[*owner as usize] += a;
        }
    }
    for (t, (owner, _, _)) in twigs.iter().enumerate() {
        area[*owner as usize] += twig_area[t];
    }
    for i in (1..n).rev() {
        if sc.parent[i] != NONE {
            let a = area[i];
            area[sc.parent[i] as usize] += a;
        }
    }
    let stems = b.stems.max(1) as f32;
    let r0 = h * b.trunk_radius_ratio * p.age.powf(0.4) / stems.sqrt();
    let root_area = (0..n)
        .filter(|&i| sc.parent[i] == NONE)
        .map(|i| area[i])
        .fold(1e-3f32, f32::max);
    let k = r0 / root_area.sqrt();
    let r_shoot = (k * 0.7).clamp(0.0015, 0.004);
    let radius = |a: f32| (k * a.max(0.0).sqrt()).max(r_shoot);

    // Chains: from each root follow the child carrying most; the others start new branches
    // one level down.
    let mut sk = Skeleton::default();
    let mut stack: Vec<(u32, u8)> = (0..n as u32)
        .filter(|&i| sc.parent[i as usize] == NONE)
        .map(|i| (i, 0u8))
        .collect();
    stack.reverse();
    while let Some((start, level)) = stack.pop() {
        let mut nodes = Vec::new();
        let par = sc.parent[start as usize];
        if par != NONE {
            let q = par as usize;
            nodes.push(Node {
                p: sc.p[q],
                r: radius(area[start as usize]),
                w: bend(dist[q]),
            });
        }
        let mut i = start as usize;
        loop {
            let mut r = radius(area[i]);
            if level == 0 {
                let f = (1.0 - sc.p[i].y / (h * 0.1).max(1e-3)).max(0.0);
                r *= 1.0 + b.trunk_flare * p.age * p.age * f * f;
            }
            nodes.push(Node {
                p: sc.p[i],
                r,
                w: if par == NONE && nodes.is_empty() {
                    0.0
                } else {
                    bend(dist[i])
                },
            });
            let Some(&main) = kids[i].iter().max_by(|&&a, &&c| {
                area[a as usize]
                    .total_cmp(&area[c as usize])
                    .then(c.cmp(&a))
            }) else {
                break;
            };
            for &c in kids[i].iter().rev() {
                if c != main {
                    stack.push((c, level.saturating_add(1)));
                }
            }
            i = main as usize;
        }
        if nodes.len() >= 2 {
            let ph = phase[start as usize];
            sk.branches.push(Branch { nodes, phase: ph });
            sk.levels.push(level);
        }
    }
    let twig_level = sk
        .levels
        .iter()
        .copied()
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    for (t, (owner, q, ph)) in twigs.iter().enumerate() {
        let wo = bend(dist[*owner as usize]);
        let r = radius(twig_area[t]);
        sk.branches.push(Branch {
            // A straight stick: one cone of a few facets.
            nodes: vec![
                Node { p: q[0], r, w: wo },
                Node {
                    p: q[2],
                    r: r_shoot,
                    w: (wo + 0.15).min(1.0),
                },
            ],
            phase: *ph,
        });
        sk.levels.push(twig_level);
    }
    // Shoots most in the light first (the order the leaf levels fill their budgets in); ties
    // by position in the growth order.
    let mut shoots: Vec<Shoot> = shoots.into_iter().map(|(_, _, s)| s).collect();
    let mut order: Vec<usize> = (0..shoots.len()).collect();
    order.sort_by(|&a, &c| shoots[c].light.total_cmp(&shoots[a].light).then(a.cmp(&c)));
    shoots = order.into_iter().map(|i| shoots[i]).collect();
    // The card levels hang their sprays on the lit shoots (shed ones carry none).
    let lit = shoots
        .iter()
        .filter(|s| s.light > 0.15)
        .count()
        .max(shoots.len().min(64));
    sk.leaves = shoots[..lit]
        .iter()
        .map(|s| LeafPoint {
            p: s.p + s.d * (s.len * 0.5),
            d: s.d,
            w: s.w,
            phase: s.phase,
        })
        .collect();
    sk.shoot_r = r_shoot;
    sk.shoots = shoots;
    sk
}

/// Branches thinner than this at their base are fine twigs ([`Builder::twig_tube`]).
pub(crate) const FINE_M: f32 = 0.012;

/// Sides of a branch's tube by its base radius, for the leaf level.
fn sides(r: f32) -> u32 {
    if r > 0.12 {
        12
    } else if r > 0.05 {
        8
    } else if r > 0.015 {
        5
    } else {
        3
    }
}

/// Triangle budgets of the two leaf levels (`crate::generate_detail`).
pub(crate) fn detail_budget(class: BudgetClass) -> [usize; 2] {
    match class {
        BudgetClass::LargeTree => [150_000, 40_000],
        BudgetClass::SmallTree => [80_000, 24_000],
        _ => [0, 0],
    }
}

/// The nearest level: every branch of the skeleton as a tube, each lit shoot a leaf cluster
/// card (atlas cells 14 and 15: a twig with its leaves at true size, in the species'
/// phyllotaxis), bare shoots as fine twigs out of leaf, within `budget` triangles.
pub(crate) fn leaf_level(
    p: &PlantParams,
    b: &BranchingParams,
    sk: &Skeleton,
    seed: u64,
    st: &SeasonState,
    budget: usize,
) -> PlantMesh {
    let mut out = Builder::default();
    // Wood first, thickest first: a quarter of the budget in leaf (the clusters draw their
    // twigs), most of it bare.
    let wood_budget = (budget as f32 * (0.25 + 0.6 * (1.0 - st.leaf_fraction))) as usize;
    let mut order: Vec<usize> = (0..sk.branches.len()).collect();
    order.sort_by(|&a, &c| {
        sk.branches[c].nodes[0]
            .r
            .total_cmp(&sk.branches[a].nodes[0].r)
            .then(a.cmp(&c))
    });
    let mut used = 0;
    for i in order {
        let br = &sk.branches[i];
        let s = sides(br.nodes[0].r);
        let cost = Builder::tube_cost(br.nodes.len(), s);
        if used + cost > wood_budget {
            continue;
        }
        used += cost;
        if br.nodes[0].r < FINE_M {
            out.twig_tube(&br.nodes, s, br.phase);
        } else {
            out.tube(&br.nodes, s, br.phase);
        }
    }
    let left = budget.saturating_sub(out.tris());
    let lo = sk
        .shoots
        .iter()
        .fold(v3(f32::MAX, f32::MAX, f32::MAX), |a, s| {
            v3(a.x.min(s.p.x), a.y.min(s.p.y), a.z.min(s.p.z))
        });
    let hi = sk
        .shoots
        .iter()
        .fold(v3(f32::MIN, f32::MIN, f32::MIN), |a, s| {
            v3(a.x.max(s.p.x), a.y.max(s.p.y), a.z.max(s.p.z))
        });
    let crown = Outward::Crown {
        centre: (lo + hi) * 0.5,
        radii: {
            let r = (hi - lo) * 0.5;
            v3(r.x.max(0.2), r.y.max(0.2), r.z.max(0.2))
        },
    };
    let cm = cluster_m(p);
    let flowers = p.flowers.as_ref().filter(|_| st.flowers);
    // Leaves on lit shoots; the season keeps a fraction of them (a hash per shoot, so the
    // same leaves stay from month to month).
    let lit = sk.shoots.iter().filter(|s| s.light > 0.15).count();
    let max_cards = left / 2;
    let mut cards = 0;
    let mut bare: Vec<usize> = Vec::new();
    for (i, s) in sk.shoots.iter().enumerate() {
        let keep = crate::math::hash2(i as i32, seed as i32, 0x1eaf) < st.leaf_fraction;
        if i >= lit || !keep || cards >= max_cards {
            bare.push(i);
            continue;
        }
        let mut r = Rng::new(seed ^ 0x1eaf, i as u64);
        // Leaves spread flat to the light, turned at random about the shoot.
        let flat = s.d.cross(UP);
        let side0 = if flat.len() > 0.2 {
            flat.norm()
        } else {
            s.d.perp()
        };
        let side = side0.rotate(s.d, r.sym() * 0.7 * (1.0 - 0.5 * b.leaf_up));
        let card = Card {
            base: s.p,
            up: s.d,
            side,
            length: cm,
            width: cm,
            cell: 14 + (i % 2) as u8,
            material: Material::Foliage,
            wind: (s.w, (s.w + 0.15).min(1.0)),
            phase: s.phase,
            // Leaves weigh the shoot down a little.
            bend: 0.25 * r.f(),
            segments: 1,
            outward_mix: 0.35,
        };
        out.card(&card, &crown);
        cards += 1;
    }
    if let Some(f) = flowers {
        let n = ((cards as f32 * f.density) as usize).min(budget.saturating_sub(out.tris()) / 4);
        for (k, s) in sk.shoots.iter().take(n).enumerate() {
            let tip = s.p + s.d * s.len;
            let mut r = Rng::new(seed ^ 0xf10e, k as u64);
            let side = s.d.perp().rotate(s.d, r.f() * TAU);
            out.crossed(
                &Card {
                    base: tip - s.d * (f.card_m * 0.3),
                    up: s.d,
                    side,
                    length: f.card_m,
                    width: f.card_m,
                    cell: 8 + (k % 4) as u8,
                    material: Material::Flower,
                    wind: (s.w, (s.w + 0.15).min(1.0)),
                    phase: s.phase,
                    bend: 0.0,
                    segments: 1,
                    outward_mix: 0.5,
                },
                &crown,
            );
        }
    }
    // Bare shoots as fine twigs (a cone of three facets each), most lit first.
    for &i in &bare {
        if out.tris() + 3 > budget {
            break;
        }
        let s = &sk.shoots[i];
        out.twig_tube(
            &[
                Node {
                    p: s.p,
                    r: sk.shoot_r,
                    w: s.w,
                },
                Node {
                    p: s.p + s.d * s.len,
                    r: sk.shoot_r * 0.5,
                    w: (s.w + 0.15).min(1.0),
                },
            ],
            3,
            s.phase,
        );
    }
    debug_assert!(out.tris() <= budget);
    out.finish()
}
