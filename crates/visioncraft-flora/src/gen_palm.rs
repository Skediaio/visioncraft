//! Single-trunk palms (Chusan / windmill palm, Canary Island date palm, date palm): a trunk, a
//! spiral crown of fan or feather fronds on petioles (golden-angle phyllotaxis, Prusinkiewicz &
//! Lindenmayer ch. 4), younger fronds upright at the top and older ones lower and drooping, a
//! skirt of dead fronds, and where the species has them a hanging bunch of dates under the
//! crown and offshoots round the foot.

use crate::math::{GOLDEN_ANGLE, Rng, TAU, UP, V3, cos, deg, sin, v3};
use crate::mesh::{Builder, Card, Node, Outward};
use crate::{Lod, Material, PalmParams, PlantMesh, PlantParams, SeasonState};

pub fn generate(
    p: &PlantParams,
    pp: &PalmParams,
    seed: u64,
    lod: Lod,
    st: &SeasonState,
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
        // Chusan palm trunks are thicker near the top where the old leaf bases cling; a date
        // palm's is the same width all the way up (FAO, "Date Palm Cultivation", ch. 3).
        let r = r0 * (1.0 + (pp.trunk_top_ratio - 1.0) * t) * if i == 0 { 1.15 } else { 1.0 };
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
    // Offshoots stand round the foot of a palm old enough to make them (in flower, never on
    // the far level: a metre at the foot of twenty is not seen from there). Each: a stub and a
    // tuft of short fronds.
    let n_suck = if p.age >= 0.3 && li < 2 {
        pp.suckers as usize
    } else {
        0
    };
    let sucker_tris = if n_suck > 0 {
        n_suck * (Builder::tube_cost(2, 3) + 2 * [8usize, 6][li])
    } else {
        0
    };
    // Dates hang in bunches under the crown: as many at the near level as the species says,
    // three fifths and a third of them further out.
    let bunches = p.flowers.as_ref().filter(|_| st.flowers);
    let n_bunch = bunches.map_or(0, |f| {
        let n = f.density.round().max(1.0) as usize;
        [n, (n * 3).div_ceil(5), n.div_ceil(3)][li]
    });
    let bunch_tris = [8usize, 4, 2][li]; // crossed, bent / crossed / one flat card
    let wood = out.tris();
    // The bunches and offshoots keep their triangles before the fronds fit.
    let extras = sucker_tris + n_bunch * bunch_tris;
    let fit = ((budget.saturating_sub(wood + extras)) / per_frond(li)).max(1);
    let n_live = fronds.min(fit);
    let n_dead = dead.min((budget.saturating_sub(wood + extras + n_live * per_frond(li))) / 2);
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
            // The youngest rise steeply, the oldest hang: a date palm's oldest hang far
            // below the horizontal where a Chusan palm's only droop.
            deg(pp.elev_young_deg + (pp.elev_old_deg - pp.elev_young_deg) * age_k + 8.0 * r.sym())
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
            bend: if li == 0 { 0.5 } else { 0.3 } + deg(pp.arch_deg) * age_k,
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
    // The bunches, on the lower fronds' side of the crown: hanging from just under the frond
    // bases, a little out from the trunk (a date palm's bunches weigh their stalks down among
    // its oldest fronds; FAO, "Date Palm Cultivation", ch. 3). The card's tip points down.
    if let Some(f) = bunches {
        let field = Outward::Axis {
            centre: top,
            up: 0.3,
        };
        let mut brng = Rng::new(seed ^ 0xDA7E, 5);
        let n = n_bunch.min(budget.saturating_sub(out.tris()) / bunch_tris);
        for j in 0..n {
            let u = brng.f();
            let a = j as f32 * GOLDEN_ANGLE + 0.6 * brng.sym();
            let out_dir = v3(cos(a), 0.0, sin(a));
            let base =
                top - UP * (0.25 + 0.35 * u) + out_dir * (r0 * 0.8 + petiole * (0.35 + 0.25 * u));
            let len = f.card_m * brng.range(0.8, 1.1);
            let card = Card {
                base,
                up: (-UP + out_dir * 0.3).norm(),
                side: out_dir.cross(UP).norm(),
                length: len,
                width: f.card_m * 0.5,
                cell: 8 + (j % 4) as u8,
                material: Material::Flower,
                wind: (0.5, 0.9),
                phase: brng.f(),
                bend: 0.0,
                segments: if li == 0 { 2 } else { 1 },
                outward_mix: 0.5,
            };
            if li == 2 {
                out.card(&card, &field);
            } else {
                out.crossed(&card, &field);
            }
        }
    }
    // Offshoots round the foot, each its own little palm: a stub with a spiral tuft of fronds.
    for j in 0..n_suck {
        let mut srng = Rng::new(seed ^ 0x5ACC, 900 + j as u64);
        let a = (j as f32 + 0.2 * srng.sym()) * TAU / n_suck.max(1) as f32;
        let out_d = v3(cos(a), 0.0, sin(a));
        let base = out_d * (r0 * srng.range(1.3, 1.8) + 0.05);
        let stub = 0.25 * age * srng.range(0.7, 1.3);
        let head = base + UP * stub;
        out.tube(
            &[
                Node {
                    p: base,
                    r: 0.07 * age,
                    w: 0.0,
                },
                Node {
                    p: head,
                    r: 0.05 * age,
                    w: 0.3,
                },
            ],
            3,
            srng.f(),
        );
        let fr = [8usize, 6][li];
        let fl = 0.24 * (blade + petiole);
        let field = Outward::Axis {
            centre: head,
            up: 0.8,
        };
        for k in 0..fr {
            let mut r = Rng::new(seed ^ 0x0FF5, 1000 + (j * 16 + k) as u64);
            let fa = k as f32 * GOLDEN_ANGLE + a;
            let u = k as f32 / fr as f32; // 0 the youngest
            let e = deg(75.0 - 85.0 * u + 8.0 * r.sym());
            let dir = (UP * sin(e) + v3(cos(fa), 0.0, sin(fa)) * cos(e)).norm();
            let l = fl * r.range(0.85, 1.15);
            let card = Card {
                base: head - UP * (0.05 * u),
                up: (dir + UP * 0.25).norm(),
                side: dir.cross(UP).norm(),
                length: l,
                width: l,
                cell: (k % 4) as u8,
                material: Material::Foliage,
                wind: (0.2, 0.8),
                phase: r.f(),
                bend: 0.4,
                segments: 1,
                outward_mix: 0.6,
            };
            out.card(&card, &field);
        }
    }
    debug_assert!(out.tris() <= budget);
    out.finish()
}
