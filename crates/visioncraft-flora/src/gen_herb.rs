//! Herbaceous and low forms: grass tussocks, basal rosettes and leafy mounds with flower
//! stalks (perennials, ferns), ground-cover patches and wall climbers.
//!
//! Leaf and blade placement uses spiral phyllotaxis with the golden angle, and ground-cover
//! patches Vogel's sunflower model r = c * sqrt(k), theta = k * 137.5 degrees (both from
//! Prusinkiewicz & Lindenmayer, "The Algorithmic Beauty of Plants", ch. 4).

use crate::math::{GOLDEN_ANGLE, Rng, TAU, UP, V3, atan2, cos, deg, sin, v3};
use crate::mesh::{Builder, Card, Node, Outward, cell_uv, rand_dir, shuffled};
use crate::{
    BudgetClass, ClimberParams, FlowerShape, Form, GroundCoverParams, LeafShape, Lod, Material,
    PlantMesh, PlantParams, RosetteParams, SeasonState, TussockParams,
};

/// Triangles of a grass's or herbaceous plant's stem level ([`crate::generate_levels`] with
/// detail): every blade, stem and spike its own geometry at true size. 0 for other forms and for
/// lawn tufts.
pub(crate) fn detail_budget(p: &PlantParams) -> usize {
    match (&p.form, p.budget) {
        (Form::Tussock(_) | Form::Rosette(_), BudgetClass::Herb) => 6_000,
        (Form::Tussock(_) | Form::Rosette(_), BudgetClass::Shrub) => 9_000,
        _ => 0,
    }
}

/// Whether `budget` is a stem level's (above the class's near level).
fn is_detail(p: &PlantParams, budget: usize) -> bool {
    budget > p.budget.triangles(Lod::Lod0)
}

/// Each seed of a plant its own size within the species: a little taller or broader, so
/// neighbours in a drift differ (own stream, the placement draws stay as they were).
fn form_of(seed: u64) -> (f32, f32) {
    let mut r = Rng::new(seed, 7);
    (r.range(0.86, 1.12), r.range(0.86, 1.14))
}

fn strip(
    out: &mut Builder,
    pts: &[V3],
    side: V3,
    width: f32,
    cell: u8,
    phase: f32,
    outward: &Outward,
    mix: f32,
) {
    let n = pts.len();
    let mut prev: Option<(u32, u32)> = None;
    for (k, &q) in pts.iter().enumerate() {
        let t = k as f32 / (n - 1) as f32;
        let tangent = (pts[(k + 1).min(n - 1)] - pts[k.saturating_sub(1)]).norm();
        let mut face = side.cross(tangent).norm();
        let o = outward.at(q);
        if face.dot(o) < 0.0 {
            face = -face;
        }
        let nrm = (o * mix + face * (1.0 - mix)).norm();
        let w = t * t.sqrt();
        let a = out.vert(
            q - side * (width * 0.5),
            nrm,
            cell_uv(cell, 0.0, t),
            [w, phase],
        );
        let b = out.vert(
            q + side * (width * 0.5),
            nrm,
            cell_uv(cell, 1.0, t),
            [w, phase],
        );
        if let Some((pa, pb)) = prev {
            out.quad(Material::Foliage, pa, pb, b, a);
        }
        prev = Some((a, b));
    }
}

pub fn tussock(
    p: &PlantParams,
    t: &TussockParams,
    seed: u64,
    lod: Lod,
    st: &SeasonState,
    budget: usize,
) -> PlantMesh {
    let li = lod.index();
    let dense = is_detail(p, budget);
    let (fh, fw) = if p.budget == BudgetClass::Tuft {
        (1.0, 1.0)
    } else {
        form_of(seed)
    };
    let h = p.height_m * p.age * fh;
    let w = p.width_m * p.age * fw;
    let hf = h * t.foliage_height;
    let plumes_on = st.flowers && p.flowers.is_some() && t.plumes > 0;
    let n_plume = if plumes_on {
        [
            t.plumes as usize,
            (t.plumes as usize * 3).div_ceil(5),
            (t.plumes as usize).div_ceil(3),
        ][li]
    } else {
        0
    };
    // stalk + (crossed, bent at LOD0) card; the stem level: a bowed stalk and three bent cards
    let plume_cost = |l: usize| {
        if dense {
            12 + 18
        } else {
            [3 + 8, 3 + 4, 3 + 2][l]
        }
    };
    let n_plume = n_plume.min(budget / 3 / plume_cost(li));
    let len0 = hf / (cos(deg(t.min_angle_deg)) - 0.45 * t.droop).max(0.35);
    let segs = if dense {
        // A piece every 10 cm or so: short fescue needles need three, so more of them fit.
        ((len0 / 0.1).ceil() as usize).clamp(3, 6)
    } else if budget < 30 {
        2
    } else {
        [4, 3, 2][li]
    };
    // Tiny budgets (far lawn tufts) get fan cards only: symmetric and cheap. The stem level has
    // none: its blades are all there, and a fan card's painted blades read as a flat cut-out.
    let fans_total = if budget < 12 {
        (budget / 2) as u16
    } else if dense {
        0
    } else {
        t.fans
    };
    let n_fan = if budget < 12 {
        fans_total as usize
    } else {
        [
            t.fans as usize,
            (t.fans as usize * 3).div_ceil(4),
            (t.fans as usize).div_ceil(2),
        ][li]
    };
    // Fan cards take at most half of the budget.
    let n_fan = n_fan.min(budget / 4);
    // Low arching cards radiating from the base, so the clump also reads in plan view.
    let top_cost = if li == 0 { 4 } else { 2 };
    let n_top = if budget < 12 || t.fans == 0 || dense {
        0
    } else {
        [10usize, 7, 5][li]
    };
    let n_top =
        n_top.min(budget.saturating_sub(n_plume * plume_cost(li) + n_fan * 2) / 2 / top_cost);
    // A grown clump has hundreds of leaves (Pennisetum alopecuroides: blades 10-45 cm by 3-6
    // mm in "graceful, spreading clumps"); the near card levels stand for them with the fans.
    let n0 = t.blades as usize * if dense { 10 } else { 1 };
    let nk = if budget < 12 {
        0
    } else {
        n0.min(
            budget.saturating_sub(n_plume * plume_cost(li) + n_fan * 2 + n_top * top_cost)
                / (2 * segs),
        )
    };
    let ratio = n0 as f32 / nk.max(1) as f32;
    let fan = ratio > 3.0;

    let mut rng = Rng::new(seed, 1);
    let min_a = deg(t.min_angle_deg);
    let max_a = deg(t.max_angle_deg);
    struct Blade {
        base: V3,
        az: f32,
        ang: f32,
        len: f32,
        phase: f32,
    }
    let blades: Vec<Blade> = (0..n0)
        .map(|i| {
            let rb = t.base_radius_m * p.age * rng.f().sqrt();
            let az = i as f32 * GOLDEN_ANGLE + 0.3 * rng.sym();
            let outer = (rb / (t.base_radius_m * p.age).max(1e-3)) * 0.5 + 0.5 * rng.f();
            Blade {
                base: v3(cos(az) * rb, 0.0, sin(az) * rb),
                az,
                ang: min_a + (max_a - min_a) * outer,
                len: len0 * rng.range(0.7, 1.0),
                phase: rng.f(),
            }
        })
        .collect();
    let curve = |b: &Blade, s: usize| -> Vec<V3> {
        let d = v3(cos(b.az) * sin(b.ang), cos(b.ang), sin(b.az) * sin(b.ang));
        let droop = t.droop * (0.4 + sin(b.ang));
        (0..=s)
            .map(|k| {
                let u = k as f32 / s as f32;
                let q = b.base + d * (b.len * u) - UP * (droop * 0.7 * b.len * u * u);
                // Tips arch over but do not dig into the ground.
                v3(q.x, q.y.max(hf * 0.04 * u), q.z)
            })
            .collect()
    };
    let out_field = Outward::Axis {
        centre: v3(0.0, hf * 0.2, 0.0),
        up: 1.0,
    };
    let mut out = Builder::default();
    let order = shuffled(n0, &mut Rng::new(seed, 2));
    let width = if dense {
        // At least 4 mm: a fescue's needles are under a pixel from 2 m, and an alpha-cut strip
        // that thin breaks up into a haze instead of reading as a tuft.
        t.blade_width_m.max(0.004)
    } else if fan {
        (t.blade_width_m * ratio).min(hf * 0.7)
    } else {
        t.blade_width_m * ratio.sqrt().min(3.0)
    };
    // Fan cards: vertical planes through the axis, together a fountain of blades from any side.
    let mut frng = Rng::new(seed, 4);
    for k in 0..fans_total as usize {
        let th = std::f32::consts::PI * k as f32 / fans_total.max(1) as f32 + 0.2 * frng.sym();
        let tilt = deg(6.0) * frng.sym();
        let ph = frng.f();
        if k >= n_fan {
            continue;
        }
        let side = v3(cos(th), 0.0, sin(th));
        let up = UP.rotate(side, tilt);
        let card = Card {
            base: V3::default(),
            up,
            side,
            length: hf * 1.05,
            width: w * 0.95,
            cell: 4 + (k % 3) as u8,
            material: Material::Foliage,
            wind: (0.0, 1.0),
            phase: ph,
            bend: 0.0,
            segments: 1,
            outward_mix: 0.65,
        };
        out.card(&card, &out_field);
    }
    for k in 0..n_top {
        let az = TAU * (k as f32 + 0.5 * frng.f()) / n_top as f32;
        let elev = deg(22.0 + 12.0 * frng.f());
        let outd = v3(cos(az), 0.0, sin(az));
        let card = Card {
            base: UP * (hf * 0.12),
            up: (outd * cos(elev) + UP * sin(elev)).norm(),
            side: v3(-sin(az), 0.0, cos(az)),
            length: w * 0.5,
            width: w * 0.45,
            cell: 4 + (k % 3) as u8,
            material: Material::Foliage,
            wind: (0.1, 1.0),
            phase: frng.f(),
            bend: 0.45,
            segments: if li == 0 { 2 } else { 1 },
            outward_mix: 0.55,
        };
        out.card(&card, &out_field);
    }
    for (k, &i) in order.iter().take(nk).enumerate() {
        let b = &blades[i];
        let pts = curve(b, segs);
        let mut side = v3(-sin(b.az), 0.0, cos(b.az));
        if dense {
            // Single blades turn about their length (keeled or folded leaves): edge-on to some
            // views, so the clump is not a set of ribbons all facing out.
            side = side.rotate((pts[segs] - pts[0]).norm(), 1.2 * (b.phase - 0.5));
        }
        let cell = if fan {
            4 + (k % 3) as u8
        } else {
            (k % 4) as u8
        };
        strip(&mut out, &pts, side, width, cell, b.phase, &out_field, 0.6);
    }
    if let Some(f) = p.flowers.as_ref().filter(|_| plumes_on) {
        // Each plume: a thin stalk rising out of the foliage and a crossed plume card on top.
        let flower_out = Outward::Axis {
            centre: v3(0.0, h * 0.5, 0.0),
            up: 0.6,
        };
        let mut prng = Rng::new(seed, 3);
        let stalk_r = (0.002 + 0.002 * h).min(0.008);
        for j in 0..t.plumes as usize {
            let az = j as f32 * GOLDEN_ANGLE + prng.sym();
            let lean = deg(t.plume_spread_deg) * prng.f().sqrt();
            let top_y = h * prng.range(0.9, 1.0);
            let ph = prng.f();
            if j >= n_plume {
                continue;
            }
            let dir = v3(cos(az) * sin(lean), cos(lean), sin(az) * sin(lean));
            let base = v3(0.0, hf * 0.3, 0.0);
            let total = (top_y - base.y) / cos(lean).max(0.3);
            let plume_len = t.plume_m.min(total * 0.8);
            let plume_base = base + dir * (total - plume_len);
            let tip = plume_base + dir * (plume_len * 0.3);
            if dense {
                // The stalk bows out under its plume: a midpoint pushed out, four sides.
                let out_d = v3(cos(az), 0.0, sin(az));
                let mid = (base + plume_base) * 0.5 + out_d * (0.04 * total);
                let n = |p, r, w| Node { p, r, w };
                out.tube(
                    &[
                        n(base, stalk_r, 0.1),
                        n(mid, stalk_r * 0.8, 0.45),
                        n(tip, stalk_r * 0.6, 0.8),
                    ],
                    4,
                    ph,
                );
            } else {
                out.tube(
                    &[
                        Node {
                            p: base,
                            r: stalk_r,
                            w: 0.1,
                        },
                        Node {
                            p: tip,
                            r: stalk_r * 0.6,
                            w: 0.8,
                        },
                    ],
                    3,
                    ph,
                );
            }
            let card = Card {
                base: plume_base,
                up: dir,
                side: v3(-sin(az), 0.0, cos(az)),
                length: plume_len,
                width: f.card_m * 1.6 * if li == 2 { 1.3 } else { 1.0 },
                cell: 8 + (j % 4) as u8,
                material: Material::Flower,
                wind: (0.8, 1.0),
                phase: ph,
                bend: if matches!(f.shape, FlowerShape::Bottlebrush) {
                    0.6
                } else {
                    0.1
                },
                segments: if dense {
                    3
                } else if li == 0 {
                    2
                } else {
                    1
                },
                outward_mix: 0.5,
            };
            if dense {
                // Three planes 60 degrees apart: from near, two crossed cards show as a flat
                // feather edge-on.
                for k in 0..3 {
                    let s = card.side.rotate(dir, TAU * k as f32 / 6.0);
                    out.card(&Card { side: s, ..card }, &flower_out);
                }
            } else if li == 2 {
                out.card(&card, &flower_out)
            } else {
                out.crossed(&card, &flower_out)
            }
        }
    }
    out.fit_from(0, w * 0.5, if plumes_on { h } else { hf });
    out.finish()
}

pub fn rosette(
    p: &PlantParams,
    r: &RosetteParams,
    seed: u64,
    lod: Lod,
    st: &SeasonState,
    budget: usize,
) -> PlantMesh {
    let li = lod.index();
    let dense = is_detail(p, budget);
    let (fh, fw) = form_of(seed);
    let h = p.height_m * p.age * fh;
    let w = p.width_m * p.age * fw;
    let hf = h * r.foliage_height;
    let flowers = p.flowers.as_ref().filter(|_| st.flowers && r.stalks > 0);
    let per_stalk = flowers.map_or(0, |f| f.density.round().max(1.0) as usize);
    let head_cost = |l: usize, f: FlowerShape| match (spike_like(f), dense) {
        (false, true) => HEAD_RAYS,
        (true, _) if l < 2 => 4,
        _ => 2,
    };
    // The stem level: a bowed three-node stalk, and two stem leaves under a flower head.
    let stalk_cost = |_: usize| {
        if !dense {
            3
        } else if flowers.is_some_and(|f| spike_like(f.shape)) {
            9
        } else {
            9 + 8
        }
    };
    // A flowering plant has many more stems than the card levels draw: lavender's slender,
    // leafless flowering stems (10-30 cm, a 2-8 cm spike on each; Wikipedia, "Lavandula
    // angustifolia") stand in their hundreds over a mature mound.
    let m0 = r.stalks as usize * if dense { 3 } else { 1 };
    let mk = if flowers.is_some() {
        [m0, (m0 * 3).div_ceil(5), m0.div_ceil(3)][li]
    } else {
        0
    };
    // Flowers may take at most half the budget; leaves keep the rest.
    let mk = flowers.map_or(0, |f| {
        mk.min(budget / 2 / (stalk_cost(li) + per_stalk * head_cost(li, f.shape)))
    });
    let flower_tris = flowers.map_or(0, |f| {
        mk * (stalk_cost(li) + per_stalk * head_cost(li, f.shape))
    });
    let segs = if dense {
        4
    } else if budget < 60 {
        1
    } else {
        [3, 2, 1][li]
    };
    // The stem level closes a mound of leafy shoots with twice as many.
    let n0 = r.leaves as usize * if dense && r.shoots { 2 } else { 1 };
    let nk = n0
        .min(budget.saturating_sub(flower_tris) / (2 * segs))
        .max(1);
    let grow = ((n0 as f32 / nk as f32).sqrt()).min(2.2);

    let mut rng = Rng::new(seed, 1);
    let out_field = Outward::Axis {
        centre: v3(0.0, hf * 0.1, 0.0),
        up: 1.2,
    };
    let mut out = Builder::default();
    let strap = matches!(p.leaves.shape, LeafShape::Linear) && !r.shoots;
    struct Leaf {
        base: V3,
        az: f32,
        elev: f32,
        len: f32,
        phase: f32,
        roll: f32,
    }
    let leaves: Vec<Leaf> = (0..n0)
        .map(|i| {
            let q = i as f32 / n0.max(1) as f32;
            let az = i as f32 * GOLDEN_ANGLE + 0.2 * rng.sym();
            // Leafy shoots of a mound rise from a crown as wide as a third of it: the upright
            // inner ones (first) in the middle, the spreading outer ones round them, so the dome
            // is closed on top instead of a star of shoots from one point with a hole there.
            let spread = if r.shoots {
                w * 0.32 * (q * (0.5 + 0.5 * rng.f())).sqrt()
            } else {
                0.02 * w * rng.f()
            };
            Leaf {
                base: v3(cos(az) * spread, 0.0, sin(az) * spread),
                az,
                elev: deg(r.max_elev_deg + (r.min_elev_deg - r.max_elev_deg) * q + 6.0 * rng.sym()),
                len: r.leaf_m * p.age * rng.range(0.78, 1.0) * (1.0 - 0.15 * (1.0 - q)),
                phase: rng.f(),
                roll: rng.sym(),
            }
        })
        .collect();
    // LOD-stable subset: keep a spread of inner and outer leaves.
    let order = shuffled(n0, &mut Rng::new(seed, 2));
    for (k, &i) in order.iter().take(nk).enumerate() {
        let l = &leaves[i];
        let dir = v3(
            cos(l.az) * cos(l.elev),
            sin(l.elev),
            sin(l.az) * cos(l.elev),
        );
        let mut side = v3(-sin(l.az), 0.0, cos(l.az));
        side = side.rotate(dir, 0.35 * l.roll).norm();
        let len = l.len * grow.sqrt();
        let width = if strap {
            len * p.leaves.aspect * 1.25 * grow.sqrt()
        } else {
            len * grow.sqrt()
        };
        let card = Card {
            base: l.base,
            up: dir,
            side,
            length: len,
            width,
            cell: if grow > 1.6 && r.shoots {
                4 + (k % 3) as u8
            } else {
                (k % 4) as u8
            },
            material: Material::Foliage,
            wind: (0.0, 0.8),
            phase: l.phase,
            bend: deg(r.arch_deg),
            segments: segs as u8,
            outward_mix: if r.shoots { 0.65 } else { 0.45 },
        };
        out.card(&card, &out_field);
    }
    out.fit_from(0, w * 0.5, hf);

    if let Some(f) = flowers {
        let mut srng = Rng::new(seed, 3);
        let flower_out = Outward::Axis {
            centre: v3(0.0, hf * 0.8, 0.0),
            up: 0.8,
        };
        let stem_w = (0.002 + 0.004 * h).min(0.012);
        let mut drng = Rng::new(seed, 6);
        let spike = spike_like(f.shape);
        for j in 0..m0 {
            let az = j as f32 * GOLDEN_ANGLE + srng.sym();
            let lean = deg(r.stalk_spread_deg) * srng.f().sqrt();
            let hs = h * srng.range(0.86, 1.0);
            let ph = srng.f();
            if j >= mk {
                continue;
            }
            let side = v3(-sin(az), 0.0, cos(az));
            let outd = v3(cos(az), 0.0, sin(az));
            // A stalk from `from` m off the axis rising `rise` m leans out no further than the
            // plant is wide: its flowers stay over its own leaves at every level, so a plant
            // narrower than its species' mean does not carry them past its edge.
            // Fewer stalks at the coarser levels carry larger heads, so a border keeps its
            // colour from 8 to 15 m instead of fading to leaves.
            let fs = f.card_m * (m0 as f32 / mk.max(1) as f32).sqrt().clamp(1.0, 1.8);
            let head = fs * if spike { 0.3 } else { 0.5 };
            let max_lean =
                |from: f32, rise: f32| atan2((0.5 * w - head - from).max(0.05 * w), rise.max(1e-3));
            if dense {
                // Stems rise from all over the leaf mound, not from one point: upright in the
                // middle, leaning out towards its edge (a fan of stems from the centre read as
                // a shaving brush), each bowing a little.
                let q = drng.f().sqrt();
                let rr = (if r.shoots { 0.36 } else { 0.12 }) * w * q;
                let y0 = hf * (0.7 - 0.45 * q * q) * if r.shoots { 1.0 } else { 0.3 };
                let base = outd * rr + UP * y0;
                let rise = (hs - y0).max(h * 0.3);
                let lean = (deg(r.stalk_spread_deg) * (0.15 + 0.85 * q) * drng.range(0.7, 1.15))
                    .min(max_lean(rr, rise));
                let dir = (UP * cos(lean) + outd * sin(lean)).norm();
                let len = rise / cos(lean).max(0.3) - if spike { f.card_m * 0.6 } else { 0.0 };
                let bow = outd * (len * 0.06 * drng.sym()) + side * (len * 0.05 * drng.sym());
                let mid = base + dir * (len * 0.5) + bow;
                let top = base + dir * len + bow * 1.4;
                let n = |p, r, w| Node { p, r, w };
                out.tube(
                    &[
                        n(base, stem_w * 0.8, 0.05),
                        n(mid, stem_w * 0.6, 0.5),
                        n(top, stem_w * 0.35, 0.9),
                    ],
                    3,
                    ph,
                );
                let tdir = (top - mid).norm();
                if !spike && !r.shoots {
                    // Alternate stem leaves on the lower stem (coneflower, daisies).
                    for k in 0..2 {
                        let a = az + 1.3 + k as f32 * 2.4;
                        let ld = (v3(cos(a), 0.0, sin(a)) * 0.8 + UP * 0.6).norm();
                        let at = base + (mid - base) * (0.35 + 0.5 * k as f32);
                        let ll = p.leaves.leaf_m * drng.range(0.55, 0.8);
                        let card = Card {
                            base: at,
                            up: ld,
                            side: ld.cross(UP).norm(),
                            length: ll,
                            width: ll * p.leaves.aspect.max(0.25) * 1.3,
                            cell: ((j + k) % 4) as u8,
                            material: Material::Foliage,
                            wind: (0.4, 0.7),
                            phase: ph,
                            bend: 0.5,
                            segments: 2,
                            outward_mix: 0.4,
                        };
                        out.card(&card, &out_field);
                    }
                }
                flower_heads(
                    &mut out,
                    f,
                    top,
                    tdir,
                    side,
                    az,
                    j,
                    per_stalk,
                    f.card_m,
                    ph,
                    li,
                    &flower_out,
                    &mut srng,
                    true,
                );
                continue;
            }
            let lean = lean.min(max_lean(0.04 * w, hs));
            let dir = v3(cos(az) * sin(lean), cos(lean), sin(az) * sin(lean));
            let base = v3(cos(az), 0.0, sin(az)) * (0.04 * w);
            let stalk_len = hs / cos(lean).max(0.3) - if spike { f.card_m * 0.6 } else { 0.0 };
            // Stalk: a thin three-sided cone (bark texture = green stem).
            let top = base + dir * stalk_len;
            out.tube(
                &[
                    Node {
                        p: base,
                        r: stem_w,
                        w: 0.0,
                    },
                    Node {
                        p: top,
                        r: stem_w * 0.4,
                        w: 0.9,
                    },
                ],
                3,
                ph,
            );
            flower_heads(
                &mut out,
                f,
                top,
                dir,
                side,
                az,
                j,
                per_stalk,
                fs,
                ph,
                li,
                &flower_out,
                &mut srng,
                false,
            );
        }
    }
    out.finish()
}

/// Triangles of a [`head`].
const HEAD_RAYS: usize = 8;

/// A flower head of the stem level: the atlas cell's flower mapped onto a shallow cone round
/// `face` instead of a flat card, its centre (a daisy's disc) standing out and its rim (the
/// rays) falling back; a coneflower's rays hang down further (Echinacea purpurea's reflexed
/// ray florets).
fn head(
    out: &mut Builder,
    f: &crate::FlowerParams,
    centre: V3,
    face: V3,
    up: V3,
    sd: V3,
    r: f32,
    cell: u8,
    ph: f32,
) {
    let fall = match f.shape {
        FlowerShape::Daisy { droop: true } => 0.55,
        FlowerShape::Daisy { droop: false } => 0.12,
        _ => 0.05,
    };
    let apex = out.vert(
        centre + face * (r * 0.2),
        face,
        cell_uv(cell, 0.5, 0.5),
        [0.9, ph],
    );
    let ring: Vec<u32> = (0..HEAD_RAYS)
        .map(|m| {
            let a = TAU * m as f32 / HEAD_RAYS as f32;
            let d = sd * cos(a) + up * sin(a);
            out.vert(
                centre + d * r - face * (r * fall),
                (face + d * (0.6 * fall)).norm(),
                cell_uv(cell, 0.5 + 0.5 * cos(a), 0.5 + 0.5 * sin(a)),
                [1.0, ph],
            )
        })
        .collect();
    for m in 0..HEAD_RAYS {
        out.tri(Material::Flower, apex, ring[m], ring[(m + 1) % HEAD_RAYS]);
    }
}

fn spike_like(f: FlowerShape) -> bool {
    matches!(
        f,
        FlowerShape::Spike | FlowerShape::Panicle | FlowerShape::Plume
    )
}

/// The `per_stalk` flower heads (`fs` across) at the top of a stalk ending at `top` along `dir`.
fn flower_heads(
    out: &mut Builder,
    f: &crate::FlowerParams,
    top: V3,
    dir: V3,
    side: V3,
    az: f32,
    j: usize,
    per_stalk: usize,
    fs: f32,
    ph: f32,
    li: usize,
    flower_out: &Outward,
    srng: &mut Rng,
    dense: bool,
) {
    for k in 0..per_stalk {
        if spike_like(f.shape) {
            let card = Card {
                base: top - dir * (fs * 0.4 + k as f32 * fs * 0.3),
                // On the stem level the spike carries on its stalk's line (no kink at the join).
                up: (dir + rand_dir(srng) * if dense { 0.02 } else { 0.1 }).norm(),
                side,
                length: fs,
                width: fs * if dense { 0.45 } else { 0.55 },
                cell: 8 + ((j + k) % 4) as u8,
                material: Material::Flower,
                wind: (0.85, 1.0),
                phase: ph,
                bend: 0.0,
                segments: 1,
                outward_mix: 0.4,
            };
            if li < 2 {
                out.crossed(&card, flower_out)
            } else {
                out.card(&card, flower_out)
            }
        } else {
            // A flower head facing up and out, on a short pedicel near the stalk top.
            let fa = az + k as f32 * 2.1;
            let outd = v3(cos(fa), 0.0, sin(fa));
            let centre = top - dir * (k as f32 * fs * 0.6) + outd * (fs * 0.35 * k.min(1) as f32);
            let face = (outd * 0.75 + UP * 0.66).norm();
            let up = (UP - face * UP.dot(face)).norm();
            let sd = up.cross(face).norm();
            if dense {
                head(
                    out,
                    f,
                    centre,
                    face,
                    up,
                    sd,
                    fs * 0.5,
                    8 + ((j + k) % 4) as u8,
                    ph,
                );
                continue;
            }
            let card = Card {
                base: centre - up * (fs * 0.5),
                up,
                side: sd,
                length: fs,
                width: fs,
                cell: 8 + ((j + k) % 4) as u8,
                material: Material::Flower,
                wind: (0.9, 1.0),
                phase: ph,
                bend: 0.0,
                segments: 1,
                outward_mix: 0.3,
            };
            out.card(&card, &Outward::Dir(face));
        }
    }
}

pub fn ground_cover(
    p: &PlantParams,
    g: &GroundCoverParams,
    seed: u64,
    _lod: Lod,
    st: &SeasonState,
    budget: usize,
) -> PlantMesh {
    let r_patch = p.width_m * p.age * 0.5;
    let n0 = g.cards as usize;
    let nk = n0.min(budget / 2).max(1);
    let grow = (n0 as f32 / nk as f32).sqrt();
    let c0 = p.leaves.card_m;
    let ck = c0 * grow;
    let flowers = p.flowers.as_ref().filter(|_| st.flowers);
    let n_flower = flowers.map_or(0, |f| {
        ((nk as f32 * f.density).round() as usize).min(nk / 2)
    });
    let n_leaf = if p.deciduous {
        (((nk - n_flower) as f32) * st.leaf_fraction).round() as usize
    } else {
        nk - n_flower
    };
    let order = shuffled(n0, &mut Rng::new(seed, 2));
    let mut out = Builder::default();
    let field = Outward::Dir(UP);
    let place = |i: usize, size: f32, r: &mut Rng| {
        // Vogel's model gives an even, non-gridded spread over the disc.
        let rad = r_patch * ((i as f32 + 0.5) / n0 as f32).sqrt();
        let th = i as f32 * GOLDEN_ANGLE;
        let rad = rad.min((r_patch - size * 0.45).max(0.0));
        let tilt = deg(g.tilt_deg * r.range(0.35, 1.0)) / grow;
        let taz = r.f() * TAU;
        let n = (UP * cos(tilt) + v3(cos(taz), 0.0, sin(taz)) * sin(tilt)).norm();
        let up = n.perp().rotate(n, r.f() * TAU);
        let centre = v3(
            cos(th) * rad,
            g.depth_m * p.age * r.range(0.3, 0.75) + size * 0.25 * sin(tilt),
            sin(th) * rad,
        );
        (centre, n, up)
    };
    for (k, &i) in order.iter().take(n_leaf).enumerate() {
        let mut r = Rng::new(seed, 100 + i as u64);
        let (centre, n, up) = place(i, ck, &mut r);
        let card = Card {
            base: centre - up * (ck * 0.5),
            up,
            side: up.cross(n).norm(),
            length: ck,
            width: ck,
            cell: if grow > 1.6 {
                4 + (k % 3) as u8
            } else {
                (k % 4) as u8
            },
            material: Material::Foliage,
            wind: (0.0, 0.25),
            phase: r.f(),
            bend: 0.0,
            segments: 1,
            outward_mix: 0.6,
        };
        out.card(&card, &field);
    }
    if let Some(f) = flowers {
        let fk = f.card_m * grow.min(2.0);
        for (k, &i) in order.iter().rev().take(n_flower).enumerate() {
            let mut r = Rng::new(seed, 5000 + i as u64);
            let (centre, _, _) = place(i, fk, &mut r);
            // Flowers sit on top of the leaves, facing the sky and a little sideways.
            let n = (UP + rand_dir(&mut r) * 0.4).norm();
            let up = n.perp().rotate(n, r.f() * TAU);
            let card = Card {
                base: centre + UP * (fk * 0.3) - up * (fk * 0.5),
                up,
                side: up.cross(n).norm(),
                length: fk,
                width: fk,
                cell: 8 + (k % 4) as u8,
                material: Material::Flower,
                wind: (0.05, 0.3),
                phase: r.f(),
                bend: 0.0,
                segments: 1,
                outward_mix: 0.4,
            };
            out.card(&card, &field);
        }
    }
    out.fit_from(0, r_patch, g.depth_m * p.age + c0 * 0.4);
    out.finish()
}

pub fn climber(
    p: &PlantParams,
    c: &ClimberParams,
    seed: u64,
    lod: Lod,
    st: &SeasonState,
    budget: usize,
) -> PlantMesh {
    let li = lod.index();
    let w = c.patch_width_m * p.age;
    let h = p.height_m * p.age;
    let mut out = Builder::default();
    let mut rng = Rng::new(seed, 1);
    // Stems climbing and branching across the wall.
    let mut stems: Vec<Vec<Node>> = Vec::new();
    for j in 0..c.stems as usize {
        let mut x = -w * 0.4
            + w * 0.8 * (j as f32 + 0.5) / c.stems as f32
            + 0.1 * w * rng.sym() / c.stems as f32;
        let top = h * rng.range(0.75, 0.95);
        let n = 9;
        let mut nodes = Vec::new();
        for k in 0..n {
            let t = k as f32 / (n - 1) as f32;
            nodes.push(Node {
                p: v3(x, t * top, 0.015),
                r: 0.014 * (1.0 - 0.6 * t) * p.age.sqrt(),
                w: 0.1 * t,
            });
            x = (x + 0.12 * w * rng.sym() / c.stems.max(1) as f32).clamp(-w * 0.45, w * 0.45);
        }
        for b in 0..3 {
            let q = nodes[2 + b * 2].p;
            let dirx = if rng.f() < 0.5 { -1.0 } else { 1.0 };
            let bl = w * rng.range(0.15, 0.3);
            let mut bn = Vec::new();
            for k in 0..4 {
                let t = k as f32 / 3.0;
                let px = (q.x + dirx * bl * t).clamp(-w * 0.48, w * 0.48);
                bn.push(Node {
                    p: v3(px, (q.y + bl * 0.6 * t).min(h * 0.97), 0.015),
                    r: 0.008 * (1.0 - 0.6 * t),
                    w: 0.1 + 0.1 * t,
                });
            }
            stems.push(bn);
        }
        stems.push(nodes);
    }
    if li < 2 {
        // Stems take at most a third of the budget.
        for s in &stems {
            if out.tris() + Builder::tube_cost(s.len(), 3) <= budget / 3 {
                out.tube(s, 3, 0.0);
            }
        }
    }
    // Backing layer: opaque leaf-mass quads just off the wall, so the patch reads as solid
    // cover behind the cut-out leaf cards (only while in leaf).
    let in_leaf = st.leaf_fraction > 0.5;
    let (bx, by) = [(4usize, 5usize), (2, 3), (1, 2)][li];
    let c0 = p.leaves.card_m;
    if in_leaf {
        let inset = c0 * 0.35;
        for gy in 0..by {
            for gx in 0..bx {
                let x0 = -w * 0.5 + inset + (w - 2.0 * inset) * gx as f32 / bx as f32;
                let x1 = -w * 0.5 + inset + (w - 2.0 * inset) * (gx + 1) as f32 / bx as f32;
                let y0 = inset + (h - 2.0 * inset) * gy as f32 / by as f32;
                let y1 = inset + (h - 2.0 * inset) * (gy + 1) as f32 / by as f32;
                let n = v3(0.0, 0.0, 1.0);
                let z = c.depth_m * 0.2;
                let wv = |y: f32| (0.3 * y / h).min(1.0);
                let a = out.vert(
                    v3(x0, y0, z),
                    n,
                    crate::mesh::cell_uv(7, 0.0, 0.0),
                    [wv(y0), 0.0],
                );
                let b = out.vert(
                    v3(x1, y0, z),
                    n,
                    crate::mesh::cell_uv(7, 1.0, 0.0),
                    [wv(y0), 0.0],
                );
                let cc = out.vert(
                    v3(x1, y1, z),
                    n,
                    crate::mesh::cell_uv(7, 1.0, 1.0),
                    [wv(y1), 0.0],
                );
                let d = out.vert(
                    v3(x0, y1, z),
                    n,
                    crate::mesh::cell_uv(7, 0.0, 1.0),
                    [wv(y1), 0.0],
                );
                out.quad(Material::Foliage, a, b, cc, d);
            }
        }
    }
    let wood = out.tris();
    let desired = (p.leaves.density * w * h / (c0 * c0)).ceil() as usize;
    let wood0 = stems
        .iter()
        .map(|s| Builder::tube_cost(s.len(), 3))
        .sum::<usize>()
        .min(p.budget.triangles(Lod::Lod0) / 3)
        + 40;
    let n0 = desired
        .min(p.budget.triangles(Lod::Lod0).saturating_sub(wood0) / 2)
        .max(1);
    let nk = n0.min(budget.saturating_sub(wood) / 2).max(1);
    let grow = (n0 as f32 / nk as f32).sqrt();
    let ck = c0 * grow;
    let flowers = p.flowers.as_ref().filter(|_| st.flowers);
    let n_flower = flowers.map_or(0, |f| {
        ((nk as f32 * f.density).round() as usize).min(nk / 2)
    });
    let n_leaf = if p.deciduous {
        ((nk - n_flower) as f32 * st.leaf_fraction).round() as usize
    } else {
        nk - n_flower
    };
    // Jittered grid over the patch, visited in one fixed shuffled order.
    let cols = ((n0 as f32 * w / h).sqrt().ceil() as usize).max(1);
    let rows = n0.div_ceil(cols);
    let order = shuffled(cols * rows, &mut Rng::new(seed, 2));
    let field = Outward::Dir(v3(0.0, 0.0, 1.0));
    let place = |i: usize, size: f32, r: &mut Rng| {
        let (gx, gy) = ((i % cols) as f32, (i / cols) as f32);
        let half = size * 0.5;
        let x = (-w * 0.5 + (gx + r.f()) * w / cols as f32)
            .clamp(-w * 0.5 + half.min(w * 0.5), w * 0.5 - half.min(w * 0.5));
        let y = ((gy + r.f()) * h / rows as f32).clamp(half, (h - half).max(half));
        (x, y)
    };
    for (k, &i) in order.iter().take(n_leaf).enumerate() {
        let mut r = Rng::new(seed, 200 + i as u64);
        let (x, y) = place(i, ck, &mut r);
        let half = ck * 0.5;
        let n = (v3(0.0, 0.0, 1.0) + rand_dir(&mut r) * (0.45 / grow) + UP * 0.15).norm();
        let up0 = (UP - n * UP.dot(n)).norm();
        let up = up0.rotate(n, deg(50.0) * r.sym());
        let side = up.cross(n).norm();
        let centre = v3(x, y, c.depth_m * r.range(0.35, 1.0));
        // Sway grows with height above the ground (the stems are fixed to the wall).
        let wv = (0.5 * (y - half) / h).max(0.0);
        let card = Card {
            base: centre - up * half,
            up,
            side,
            length: ck,
            width: ck,
            cell: if grow > 1.6 {
                4 + (k % 3) as u8
            } else {
                (k % 4) as u8
            },
            material: Material::Foliage,
            wind: (wv, (wv + 0.5 * ck / h + 0.1).min(1.0)),
            phase: r.f(),
            bend: 0.0,
            segments: 1,
            outward_mix: 0.6,
        };
        out.card(&card, &field);
    }
    if let Some(f) = flowers {
        let fk = f.card_m * grow.min(2.0);
        let hanging = matches!(f.shape, FlowerShape::Raceme | FlowerShape::Panicle);
        for (k, &i) in order.iter().rev().take(n_flower).enumerate() {
            let mut r = Rng::new(seed, 7000 + i as u64);
            let (x, y) = place(i, fk, &mut r);
            let n = (v3(0.0, 0.0, 1.0) + rand_dir(&mut r) * 0.3).norm();
            // Racemes hang from their stalk: the card's tip points down.
            let up = if hanging {
                (-UP + rand_dir(&mut r) * 0.15).norm()
            } else {
                (UP - n * UP.dot(n)).norm().rotate(n, r.sym())
            };
            let side = up.cross(n).norm();
            let up = n.cross(side).norm();
            let centre = v3(x, y, c.depth_m * 1.1);
            let wv = (0.5 * (y - fk * 0.5) / h).max(0.0);
            let card = Card {
                base: centre - up * (fk * 0.5),
                up,
                side,
                length: fk,
                width: if hanging { fk * 0.55 } else { fk },
                cell: 8 + (k % 4) as u8,
                material: Material::Flower,
                wind: (wv, (wv + 0.3).min(1.0)),
                phase: r.f(),
                bend: 0.0,
                segments: 1,
                outward_mix: 0.5,
            };
            out.card(&card, &field);
        }
    }
    out.finish()
}
