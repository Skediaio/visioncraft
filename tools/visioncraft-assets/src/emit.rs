//! Where a light fixture's emitter is, found from the geometry and material names.

use crate::scene::{Alpha, Model};

pub struct Emitter {
    /// Relative to the asset pivot, metres, Y up.
    pub offset: [f32; 3],
    /// Materials made emissive and renamed `<name>_emitter`; empty when the box fallback is used.
    pub mats: Vec<usize>,
    pub how: String,
}

/// The material name without the asset id in front of it.
fn suffix(name: &str, id: &str) -> String {
    let n = name.to_lowercase();
    n.strip_prefix(&id.to_lowercase())
        .map_or(n.clone(), |s| s.trim_start_matches('_').to_string())
}

fn has_token(s: &str, words: &[&str]) -> bool {
    s.split('_').any(|t| words.contains(&t))
}

/// The emitter, first match wins: materials named bulb, flame, lamp(s), light, globe or emitter
/// (apart from the body material that carries the asset's own name); glass, shade, lens, diffuser
/// or blended materials; else the vertex mean (the centre of the upper third for candles). A
/// wall fixture's emitter is moved to at least 3 cm in front of its back plate (the flat side
/// that faces away from the room).
pub fn find(model: &Model, id: &str, wall: bool) -> Emitter {
    let used: Vec<usize> = {
        let mut u: Vec<usize> = model.meshes.iter().map(|m| m.mat).collect();
        u.sort_unstable();
        u.dedup();
        u
    };
    let named = |words: &[&str]| -> Vec<usize> {
        used.iter()
            .copied()
            .filter(|&i| {
                let n = &model.mats[i].name;
                n.to_lowercase() != id.to_lowercase() && has_token(&suffix(n, id), words)
            })
            .collect()
    };
    let mut mats = named(&[
        "bulb", "flame", "lamp", "lamps", "light", "globe", "emitter",
    ]);
    let mut how = "material name (bulb, flame, lamp, light or globe)".to_string();
    if mats.is_empty() {
        mats = used
            .iter()
            .copied()
            .filter(|&i| {
                model.mats[i].alpha == Alpha::Blend
                    || has_token(
                        &suffix(&model.mats[i].name, id),
                        &["glass", "shade", "lens", "diffuser"],
                    )
            })
            .collect();
        how = "glass or blended material".into();
    }
    let (lo, hi) = model.bounds();
    let mut offset = [
        (lo[0] + hi[0]) / 2.0,
        lo[1] + (hi[1] - lo[1]) * 5.0 / 6.0,
        (lo[2] + hi[2]) / 2.0,
    ];
    if mats.is_empty() && !id.contains("candle") {
        // hanging fixtures: the body holds most of the vertices, the chain and canopy few
        let n = model
            .meshes
            .iter()
            .map(|m| m.pos.len())
            .sum::<usize>()
            .max(1) as f32;
        let mut mean = [0.0_f32; 3];
        for p in model.meshes.iter().flat_map(|m| &m.pos) {
            (0..3).for_each(|k| mean[k] += p[k] / n);
        }
        offset = mean;
        how = "fallback: mean of the vertices (the fixture's body)".into();
    } else if mats.is_empty() {
        how = "fallback: centre of the upper third of the bounding box".into();
    } else {
        let (mut elo, mut ehi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in model
            .meshes
            .iter()
            .filter(|m| mats.contains(&m.mat))
            .flat_map(|m| &m.pos)
        {
            for k in 0..3 {
                elo[k] = elo[k].min(p[k]);
                ehi[k] = ehi[k].max(p[k]);
            }
        }
        offset = std::array::from_fn(|k| (elo[k] + ehi[k]) / 2.0);
    }
    if wall && let Some((axis, sign)) = back_plate(model) {
        let back = if sign > 0.0 { hi[axis] } else { lo[axis] };
        let front = (back - offset[axis]) * sign;
        if front < 0.03 {
            offset[axis] = back - sign * 0.03;
            how += "; moved 3 cm in front of the back plate";
        }
    }
    Emitter { offset, mats, how }
}

/// The horizontal direction (axis 0 or 2, sign) in which the largest flat faces point at the
/// extreme of the model: that side is the wall.
fn back_plate(model: &Model) -> Option<(usize, f32)> {
    let (lo, hi) = model.bounds();
    let mut best: Option<(f64, usize, f32)> = None;
    for axis in [0usize, 2] {
        for sign in [1.0_f32, -1.0] {
            let extent = hi[axis] - lo[axis];
            let edge = if sign > 0.0 { hi[axis] } else { lo[axis] };
            let mut area = 0.0_f64;
            for m in &model.meshes {
                for t in m.idx.as_chunks::<3>().0 {
                    let [a, b, c] = t.map(|i| m.pos[i as usize]);
                    let u: [f32; 3] = std::array::from_fn(|k| b[k] - a[k]);
                    let v: [f32; 3] = std::array::from_fn(|k| c[k] - a[k]);
                    let n = [
                        u[1] * v[2] - u[2] * v[1],
                        u[2] * v[0] - u[0] * v[2],
                        u[0] * v[1] - u[1] * v[0],
                    ];
                    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                    let centre = (a[axis] + b[axis] + c[axis]) / 3.0;
                    if len > 1e-12
                        && n[axis].abs() / len > 0.9
                        && (centre - edge).abs() <= 0.15 * extent
                        && n[axis].signum() == sign
                    {
                        area += f64::from(len) / 2.0;
                    }
                }
            }
            if best.is_none_or(|b| area > b.0) {
                best = Some((area, axis, sign));
            }
        }
    }
    best.filter(|b| b.0 > 0.0).map(|b| (b.1, b.2))
}

/// Make the emitter materials glow (warm white unless the source already gives a colour) and
/// name them `<name>_emitter`, so the app can tell them; every other material loses the source's
/// emissive factor, whose emissive texture is not carried over.
pub fn apply(model: &mut Model, emitter: &[usize]) {
    for (i, m) in model.mats.iter_mut().enumerate() {
        if emitter.contains(&i) {
            if m.emissive.iter().all(|&e| e <= 0.0) {
                m.emissive = [1.0, 0.82, 0.6];
                m.emissive_strength = 3.0;
            }
            if !m.name.ends_with("_emitter") {
                m.name = format!("{}_emitter", m.name);
            }
        } else {
            m.emissive = [0.0; 3];
            m.emissive_strength = 1.0;
        }
    }
}
