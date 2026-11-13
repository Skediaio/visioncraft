//! Single-trunk palms (Chusan / windmill palm): a fibrous trunk, a spiral crown of fan fronds on
//! petioles (golden-angle phyllotaxis, Prusinkiewicz & Lindenmayer ch. 4), younger fronds
//! upright at the top and older ones lower and drooping, and a skirt of dead fronds.

use crate::math::{GOLDEN_ANGLE, Rng, UP, V3, cos, deg, sin, v3};
use crate::mesh::{Builder, Card, Node, Outward};
use crate::{Lod, Material, PalmParams, PlantMesh, PlantParams, SeasonState};

pub fn generate(
    p: &PlantParams,
    pp: &PalmParams,
    seed: u64,
    lod: Lod,
    _st: &SeasonState,
    budget: usize,
) -> PlantMesh {
    let li = lod.index();
    let age = p.age;
    let h = p.height_m * age;
    let blade = pp.blade_m * age.sqrt();
    let petiole = pp.petiole_m * age.sqrt();
    let mut rng = Rng::new(seed, 1);
    // Trunk: crown top sits below the plant top by roughly the upright fronds' reach.
    let trunk_h = (h - 0.75 * (petiole + blade)).max(h * 0.3);
    let lean = deg(pp.lean_deg) * rng.f();
    let az = rng.f() * std::f32::consts::TAU;
    let segs = [14usize, 7, 3][li];
    let mut nodes = Vec::new();
    let mut d = v3(cos(az) * sin(lean), cos(lean), sin(az) * sin(lean));
    let mut q = V3::default();
    let r0 = pp.trunk_radius_m * age;
    for i in 0..=segs {
        let t = i as f32 / segs as f32;
        // Chusan palm trunks are thicker near the top where the old leaf bases cling.
        let r = r0 * (1.0 + 0.25 * t) * if i == 0 { 1.15 } else { 1.0 };
        nodes.push(Node {
            p: q,
            r,
            w: 0.3 * t * t,
        });
        q += d * (trunk_h / segs as f32);
        d = (d + v3(rng.sym(), 0.0, rng.sym()) * 0.02).norm();
    }
    let top = nodes[segs].p;
    let phase = rng.f();
    let mut out = Builder::default();
    out.tube(&nodes, [10, 7, 5][li], phase);

    let crown = Outward::Ellipsoid {
        centre: top,
        radii: v3(blade + petiole, (blade + petiole) * 0.7, blade + petiole),
    };
    let fronds = pp.fronds as usize;
    let dead = pp.dead_fronds as usize;
    let per_frond = |l: usize| [9 + 4, 9 + 2, 2][l]; // petiole tube + fan card
    let wood = out.tris();
    let fit = ((budget.saturating_sub(wood)) / per_frond(li)).max(1);
    let n_live = fronds.min(fit);
    let n_dead = dead.min((budget.saturating_sub(wood + n_live * per_frond(li))) / 2);
    let scale = ((fronds as f32 / n_live as f32).sqrt()).min(1.6);
    for k in 0..fronds + dead {
        let mut r = Rng::new(seed ^ 0xF00D, 100 + k as u64);
        let is_dead = k >= fronds;
        let idx = if is_dead { k - fronds } else { k };
        if (!is_dead && idx >= n_live) || (is_dead && idx >= n_dead) {
            continue;
        }
        let a = k as f32 * GOLDEN_ANGLE;
        let age_k = if is_dead {
            1.0
        } else {
            k as f32 / fronds.max(1) as f32
        }; // 0 youngest
        let elev = if is_dead {
            deg(-70.0 + 10.0 * r.sym())
        } else {
            deg(70.0 - 95.0 * age_k + 8.0 * r.sym())
        };
        let out_dir = v3(cos(a), 0.0, sin(a));
        let dir = (UP * sin(elev) + out_dir * cos(elev)).norm();
        let base = top + UP * (-0.15 * age_k) + out_dir * (r0 * 0.6);
        let pet = if is_dead {
            petiole * 0.6
        } else {
            petiole * (0.85 + 0.3 * r.f())
        };
        let pend = base + dir * pet + UP * (-0.08 * pet * age_k);
        let w_tip = 1.0;
        if li < 2 && !is_dead {
            let mid = base.lerp(pend, 0.5) + UP * (0.03 * pet);
            let pn = [
                Node {
                    p: base,
                    r: 0.025 * age,
                    w: 0.3,
                },
                Node {
                    p: mid,
                    r: 0.018 * age,
                    w: 0.55,
                },
                Node {
                    p: pend,
                    r: 0.012 * age,
                    w: 0.75,
                },
            ];
            out.tube(&pn, 3, (phase + 0.1 * r.f()) % 1.0);
        }
        // The fan: held roughly perpendicular to the petiole's vertical plane.
        let size = blade * if is_dead { 0.8 } else { scale };
        let side = dir.cross(UP).norm();
        let fan_up = (dir + UP * 0.25).norm();
        let start = pend - fan_up * (size * 0.08);
        let card = Card {
            base: start,
            up: fan_up,
            side,
            length: size,
            width: size,
            cell: if is_dead {
                12 + (k % 2) as u8
            } else {
                (k % 4) as u8
            },
            material: Material::Foliage,
            wind: (0.6, w_tip),
            phase: (phase + 0.2 * r.f()) % 1.0,
            bend: if li == 0 { 0.5 } else { 0.3 },
            segments: if li == 0 { 2 } else { 1 },
            outward_mix: 0.6,
        };
        if is_dead {
            out.card(
                &Card {
                    up: dir,
                    bend: 0.0,
                    segments: 1,
                    wind: (0.3, 0.6),
                    ..card
                },
                &crown,
            );
        } else if li == 0 {
            // Slight roll so the fans do not all face the same way.
            let s1 = side.rotate(fan_up, 0.3 * r.sym());
            out.card(&Card { side: s1, ..card }, &crown);
        } else {
            out.card(&card, &crown);
        }
    }
    debug_assert!(out.tris() <= budget);
    out.finish()
}
