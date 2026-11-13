//! Clipped forms (box ball, yew cone, hedge segments) and dense natural columns (thuja,
//! Italian cypress): an opaque core surface textured with the tileable leaf-mass cell, shingled
//! with foliage cards that stand slightly out of it. The core makes the form read as solid
//! from every distance; the cards break up the silhouette.
//!
//! Hedge segments run along X from -L/2 to L/2. The surface noise is periodic in X with period
//! L, so the end cross-sections are identical and segments tile end to end; cards near the ends
//! may overhang into the neighbour, as real hedge shoots do.

use crate::math::{Rng, TAU, UP, V3, cos, lerp, sin, tile_noise, v3};
use crate::mesh::{Builder, Card, Node, Outward};
use crate::{ClipShape, ClippedParams, Lod, Material, PlantMesh, PlantParams, SeasonState};

const MASS: u8 = 7;

/// Revolved profile radius relative to the half width, for t in [0, 1] up the form.
fn profile(shape: ClipShape, t: f32) -> f32 {
    let arc = |s: f32| (1.0 - s * s).max(0.0).sqrt();
    match shape {
        ClipShape::Ball => arc(2.0 * t - 1.0),
        ClipShape::Cone => (1.0 - t) * (0.82 + 0.18 * (t / 0.08).min(1.0)),
        ClipShape::Column => {
            if t < 0.78 {
                0.9 + 0.1 * (t / 0.2).min(1.0)
            } else {
                arc((t - 0.78) / 0.22)
            }
        }
        ClipShape::Flame => {
            if t < 0.28 {
                0.72 + 0.28 * (t / 0.28)
            } else {
                let s = (1.0 - t) / 0.72;
                s * (1.35 - 0.35 * s)
            }
        }
        ClipShape::HedgeSegment { .. } => 1.0,
    }
}

struct Surf {
    p: V3,
    n: V3,
}

pub fn generate(
    p: &PlantParams,
    c: &ClippedParams,
    seed: u64,
    lod: Lod,
    st: &SeasonState,
    budget: usize,
) -> PlantMesh {
    let li = lod.index();
    let h = p.height_m * p.age;
    let w = p.width_m * p.age;
    let mut out = Builder::default();
    let tuft = c.tuft_m;
    let c0 = p.leaves.card_m;
    let wind_at = |y: f32| 0.12 * (y / h).clamp(0.0, 1.0);

    // Core surface: a rows x cols grid over the parametric surface `surf`.
    let (rows, cols) = (ROWS[li], cols_for(c, li));
    let length = if let ClipShape::HedgeSegment { length_m } = c.shape {
        length_m
    } else {
        0.0
    };
    let y0 = c.stem_m * p.age;
    let core_r = (w * 0.5 - tuft * 0.75).max(w * 0.2);
    let rough = c.roughness;
    let surf = |a: f32, b: f32| -> V3 {
        // a: around (revolved) or along X (hedge), 0..1; b: up the profile, 0..1.
        match c.shape {
            ClipShape::HedgeSegment { .. } => {
                let x = (a - 0.5) * length;
                let (z, y) = hedge_profile(b, core_r, h - tuft * 0.6);
                let nz = tile_noise(a, b, 4, 5, 77) - 0.5;
                let (dz, dy) = hedge_normal(b, core_r, h - tuft * 0.6);
                // No displacement at the foot, so the hedge stands exactly on the ground.
                let k = nz * rough * 2.0 * (b * (1.0 - b) * 8.0).min(1.0);
                v3(x, y + dy * k, z + dz * k)
            }
            shape => {
                let t = b;
                let r = core_r
                    * profile(shape, t)
                    * (1.0 + (tile_noise(a, t, 7, 5, 78) - 0.5) * 2.0 * rough / core_r.max(0.05));
                let th = a * TAU;
                v3(cos(th) * r, y0 + t * (h - y0 - tuft * 0.6), sin(th) * r)
            }
        }
    };
    let normal = |a: f32, b: f32| -> V3 {
        let e = 1e-3;
        let da = surf(a + e, b) - surf(a - e, b);
        let db = surf(a, (b + e).min(1.0)) - surf(a, (b - e).max(0.0));
        let n = db.cross(da).norm();
        let q = surf(a, b);
        let centre = match c.shape {
            ClipShape::HedgeSegment { .. } => v3(q.x, h * 0.45, 0.0),
            _ => v3(0.0, q.y.min(y0 + (h - y0) * 0.5), 0.0),
        };
        if n.dot(q - centre) < 0.0 { -n } else { n }
    };

    // Core: each grid quad maps the whole tileable mass cell.
    let uv = |s: f32, t: f32| crate::mesh::cell_uv(MASS, s, t);
    for i in 0..rows {
        for j in 0..cols {
            let (a0, a1) = (j as f32 / cols as f32, (j + 1) as f32 / cols as f32);
            let (b0, b1) = (i as f32 / rows as f32, (i + 1) as f32 / rows as f32);
            let corners = [
                (a0, b0, 0.0, 0.0),
                (a1, b0, 1.0, 0.0),
                (a1, b1, 1.0, 1.0),
                (a0, b1, 0.0, 1.0),
            ];
            let is_revolved = !matches!(c.shape, ClipShape::HedgeSegment { .. });
            // Winding (0, 3, 2, 1) is counter-clockwise seen from outside for both layouts.
            let mut ids = [0u32; 4];
            for (k, &(a, b, s, t)) in corners.iter().enumerate() {
                let q = surf(a, b);
                ids[k] = out.vert(
                    q,
                    normal(a, b.clamp(0.002, 0.998)),
                    uv(s, t),
                    [wind_at(q.y), 0.0],
                );
            }
            let top_apex = is_revolved && profile(c.shape, b1) < 1e-3;
            let bot_apex = is_revolved && profile(c.shape, b0) < 1e-3;
            if top_apex {
                out.tri(Material::Foliage, ids[0], ids[3], ids[1]);
            } else if bot_apex {
                out.tri(Material::Foliage, ids[0], ids[3], ids[2]);
            } else {
                out.quad(Material::Foliage, ids[0], ids[3], ids[2], ids[1]);
            }
        }
    }
    if let ClipShape::HedgeSegment { .. } = c.shape {
        // End caps: fans over the exact same profile at x = -L/2 and x = +L/2.
        for (a, sgn) in [(0.0f32, -1.0f32), (1.0, 1.0)] {
            let centre_pt = v3((a - 0.5) * length, h * 0.4, 0.0);
            let cid = out.vert(
                centre_pt,
                v3(sgn, 0.0, 0.0),
                uv(0.5, 0.5),
                [wind_at(centre_pt.y), 0.0],
            );
            let ring: Vec<u32> = (0..=rows)
                .map(|i| {
                    let b = i as f32 / rows as f32;
                    let q = surf(a, b);
                    out.vert(
                        q,
                        v3(sgn, 0.0, 0.0),
                        uv(0.5 + q.z / (2.0 * w), 1.0 - q.y / h),
                        [wind_at(q.y), 0.0],
                    )
                })
                .collect();
            for k in 0..rows {
                if sgn > 0.0 {
                    out.tri(Material::Foliage, cid, ring[k], ring[k + 1]);
                } else {
                    out.tri(Material::Foliage, cid, ring[k + 1], ring[k]);
                }
            }
        }
    }
    if c.stem_m > 0.0 {
        let sr = (w * 0.05).max(0.02);
        let top = y0 + (h - y0) * 0.25;
        let nodes = [
            Node {
                p: V3::default(),
                r: sr,
                w: 0.0,
            },
            Node {
                p: v3(0.0, top * 0.5, 0.0),
                r: sr * 0.9,
                w: 0.02,
            },
            Node {
                p: v3(0.0, top, 0.0),
                r: sr * 0.7,
                w: 0.05,
            },
        ];
        out.tube(&nodes, [6, 4, 3][li], 0.0);
    }

    // Shell cards, sampled uniformly over the surface parameters (area-weighted by rejection).
    let core_tris = out.tris();
    let area_est = surface_area(&surf);
    let desired = ((p.leaves.density * area_est / (c0 * c0)).ceil() as usize).max(4);
    let cap = |l: usize| {
        p.budget
            .triangles(Lod::ALL[l])
            .saturating_sub(core_cost(c, l, ROWS[l], cols_for(c, l)))
            / 2
    };
    let n0 = desired.min(cap(0)).max(1);
    let nk = n0.min(budget.saturating_sub(core_tris) / 2).max(1);
    let grow = (n0 as f32 / nk as f32).sqrt();
    let ck = c0 * grow;
    let n_leaf =
        (nk as f32 * st.leaf_fraction.max(if p.deciduous { 0.0 } else { 1.0 })).round() as usize;
    let cells: [u8; 3] = if grow < 1.5 { [0, 1, 2] } else { [4, 5, 6] };
    let mut samples = Vec::with_capacity(n0);
    let mut rng = Rng::new(seed, 7);
    let max_scale = max_area_scale(&surf);
    let mut tries = 0;
    while samples.len() < n0 && tries < n0 * 40 {
        tries += 1;
        let (a, b) = (rng.f(), rng.f());
        let s = local_area_scale(&surf, a, b);
        if rng.f() * max_scale > s {
            continue;
        }
        samples.push(Surf {
            p: surf(a, b),
            n: normal(a, b.clamp(0.01, 0.99)),
        });
    }
    for (k, sp) in samples.iter().take(n_leaf.min(nk)).enumerate() {
        let mut r = Rng::new(seed ^ 0xC11, 500 + k as u64);
        let tangent = sp.n.perp().rotate(sp.n, r.f() * TAU);
        let up = (tangent * 0.75 + sp.n * 0.65 + UP * 0.2).norm();
        let side = up.cross(sp.n).norm();
        let mut centre = sp.p + sp.n * (tuft * 0.45 - (ck - c0) * 0.35);
        if let ClipShape::HedgeSegment { length_m } = c.shape {
            let lim = (length_m * 0.5 + c0 * 0.5 - ck * 0.5).max(0.0);
            centre.x = centre.x.clamp(-lim, lim);
        }
        let wy = wind_at(centre.y);
        let card = Card {
            base: centre - up * (ck * 0.5),
            up,
            side,
            length: ck,
            width: ck,
            cell: cells[k % 3],
            material: Material::Foliage,
            wind: (wy, wy + 0.1),
            phase: r.f(),
            bend: 0.0,
            segments: 1,
            outward_mix: 0.8,
        };
        out.card(&card, &Outward::Dir(sp.n));
    }
    debug_assert!(out.tris() <= budget, "{} > {budget}", out.tris());
    out.finish()
}

const ROWS: [usize; 3] = [10, 6, 4];

fn cols_for(c: &ClippedParams, l: usize) -> usize {
    match c.shape {
        ClipShape::HedgeSegment { .. } => [8, 4, 1][l],
        _ => [14, 9, 6][l],
    }
}

/// Triangles of the core (and stem) at a level, without building it.
fn core_cost(c: &ClippedParams, l: usize, rows: usize, cols: usize) -> usize {
    let stem = if c.stem_m > 0.0 {
        Builder::tube_cost(3, [6, 4, 3][l])
    } else {
        0
    };
    match c.shape {
        ClipShape::HedgeSegment { .. } => rows * cols * 2 + 2 * rows + stem,
        shape => {
            let apex = |b: f32| profile(shape, b) < 1e-3;
            let mut t = 0;
            for i in 0..rows {
                let (b0, b1) = (i as f32 / rows as f32, (i + 1) as f32 / rows as f32);
                t += cols * if apex(b0) || apex(b1) { 1 } else { 2 };
            }
            t + stem
        }
    }
}

/// Hedge cross-section at arc parameter b in [0, 1]: up the -Z side, over the rounded top, down
/// the +Z side. Returns (z, y). Half width `hw` at the base with a 12 % batter.
fn hedge_profile(b: f32, hw: f32, h: f32) -> (f32, f32) {
    let top_hw = hw * 0.88;
    let rr = (top_hw * 0.45).min(h * 0.2);
    // Segment lengths: side, quarter arc, flat top, quarter arc, side.
    let side = ((h - rr) * (h - rr) + (hw - top_hw) * (hw - top_hw)).sqrt();
    let arc = rr * std::f32::consts::FRAC_PI_2;
    let flat = 2.0 * (top_hw - rr);
    let total = 2.0 * side + 2.0 * arc + flat;
    let mut s = b.clamp(0.0, 1.0) * total;
    let side_pt = |f: f32| (lerp(hw, top_hw, f), f * (h - rr));
    if s <= side {
        let (z, y) = side_pt(s / side);
        return (-z, y);
    }
    s -= side;
    if s <= arc {
        let a = s / rr; // 0..pi/2 from the -Z side to the top
        return (-(top_hw - rr) - rr * cos(a), h - rr + rr * sin(a));
    }
    s -= arc;
    if s <= flat {
        return (-(top_hw - rr) + s, h);
    }
    s -= flat;
    if s <= arc {
        let a = s / rr;
        return ((top_hw - rr) + rr * sin(a), h - rr + rr * cos(a));
    }
    s -= arc;
    let (z, y) = side_pt(1.0 - (s / side).min(1.0));
    (z, y)
}

fn hedge_normal(b: f32, hw: f32, h: f32) -> (f32, f32) {
    let e = 1e-3;
    let (z0, y0) = hedge_profile((b - e).max(0.0), hw, h);
    let (z1, y1) = hedge_profile((b + e).min(1.0), hw, h);
    let (dz, dy) = (z1 - z0, y1 - y0);
    let l = (dz * dz + dy * dy).sqrt().max(1e-9);
    // Rotate the tangent by -90 degrees: outward for the -Z..top..+Z traversal.
    (-dy / l, dz / l)
}

fn local_area_scale(surf: &impl Fn(f32, f32) -> V3, a: f32, b: f32) -> f32 {
    let e = 1e-3;
    let b = b.clamp(e, 1.0 - e);
    let da = surf(a + e, b) - surf(a - e, b);
    let db = surf(a, b + e) - surf(a, b - e);
    da.cross(db).len() / (4.0 * e * e)
}

fn max_area_scale(surf: &impl Fn(f32, f32) -> V3) -> f32 {
    let mut m: f32 = 1e-6;
    for i in 0..24 {
        for j in 0..24 {
            m = m.max(local_area_scale(
                surf,
                (j as f32 + 0.5) / 24.0,
                (i as f32 + 0.5) / 24.0,
            ));
        }
    }
    m * 1.1
}

fn surface_area(surf: &impl Fn(f32, f32) -> V3) -> f32 {
    let n = 24;
    let mut a = 0.0;
    for i in 0..n {
        for j in 0..n {
            a += local_area_scale(
                surf,
                (j as f32 + 0.5) / n as f32,
                (i as f32 + 0.5) / n as f32,
            );
        }
    }
    a / (n * n) as f32
}
