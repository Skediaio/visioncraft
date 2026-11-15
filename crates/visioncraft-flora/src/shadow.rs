//! A tree's shadow caster: a handful of closed low-poly blobs that follow the crown plus a
//! trunk prism, with no texture. The renderer draws it into shadow maps for every level past
//! the finest (instead of the impostor's cards, which smeared long streaks over the lawn),
//! dithered by [`ShadowCaster::density`] so a sparse crown does not cast a solid black blob.

use crate::{Material, MeshPart, PlantMesh, PlantParams, Season, SeasonState};

/// Blobs a crown is split into.
pub const BLOBS: usize = 8;
/// Triangles of the caster at most: the blobs' octahedra and the trunk's six sides.
pub const MAX_TRIANGLES: usize = BLOBS * 8 + 12;
/// Smallest and largest share of a crown's shadow that is solid.
const DENSITY: (f32, f32) = (0.2, 0.75);

/// The caster's mesh in the plant's own space (every vertex's `uv.x` is `density`, which the
/// shadow shader dithers by: the share of shadow-map texels it covers).
pub struct ShadowCaster {
    pub part: MeshPart,
    pub density: f32,
}

type P = [f32; 3];

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Squared distance with the height counted half again: blobs stack up a crown more than they
/// spread across it.
fn d2(a: P, b: P) -> f32 {
    let d = sub(a, b);
    d[0] * d[0] + 2.25 * d[1] * d[1] + d[2] * d[2]
}

/// The caster of `mesh` (a far level of a tree: its leaf and twig vertices give the crown, its
/// bark the trunk), `None` for a plant with no crown.
pub fn shadow_caster(
    params: &PlantParams,
    mesh: &PlantMesh,
    season: Option<Season>,
) -> Option<ShadowCaster> {
    let mut crown: Vec<usize> = Vec::new();
    let mut bark: Vec<usize> = Vec::new();
    for p in &mesh.parts {
        let list = if p.material == Material::Bark {
            &mut bark
        } else {
            &mut crown
        };
        list.extend(
            mesh.indices[p.index_range.start as usize..p.index_range.end as usize]
                .iter()
                .map(|&i| i as usize),
        );
    }
    crown.sort_unstable();
    crown.dedup();
    if crown.len() < BLOBS * 2 {
        return None;
    }
    let pts: Vec<P> = crown.iter().map(|&i| mesh.positions[i]).collect();
    // Farthest-point seeds (the first: the highest point), then a few k-means steps.
    let highest = pts
        .iter()
        .copied()
        .fold(pts[0], |a, p| if p[1] > a[1] { p } else { a });
    let mut centres = vec![highest];
    while centres.len() < BLOBS {
        let near = |p: P| centres.iter().map(|&c| d2(p, c)).fold(f32::MAX, f32::min);
        let far = pts
            .iter()
            .copied()
            .max_by(|a, b| near(*a).total_cmp(&near(*b)))
            .unwrap_or(pts[0]);
        centres.push(far);
    }
    let nearest = |p: P, c: &[P]| {
        (0..c.len())
            .min_by(|&a, &b| d2(p, c[a]).total_cmp(&d2(p, c[b])))
            .unwrap_or(0)
    };
    for _ in 0..4 {
        let mut sum = vec![([0.0f32; 3], 0u32); BLOBS];
        for &p in &pts {
            let s = &mut sum[nearest(p, &centres)];
            (0..3).for_each(|k| s.0[k] += p[k]);
            s.1 += 1;
        }
        for (c, (s, n)) in centres.iter_mut().zip(&sum) {
            if *n > 0 {
                *c = s.map(|v| v / *n as f32);
            }
        }
    }
    let mut part = MeshPart::default();
    let mut lowest = f32::MAX;
    for (b, &centre) in centres.iter().enumerate() {
        let members: Vec<usize> = (0..pts.len())
            .filter(|&i| nearest(pts[i], &centres) == b)
            .collect();
        if members.is_empty() {
            continue;
        }
        let mut r = [0.15f32; 3];
        let mut w = 0.0;
        for &i in &members {
            let d = sub(pts[i], centre);
            (0..3).for_each(|k| r[k] = r[k].max(d[k].abs() * 0.95));
            w += mesh.wind[crown[i]][0];
        }
        lowest = lowest.min((centre[1] - r[1]).max(0.0));
        let wind = [w / members.len() as f32, mesh.wind[crown[members[0]]][1]];
        octahedron(&mut part, centre, r, wind, b as u32);
    }
    // Trunk: a six-sided prism to the crown's underside (at most half the tree's height), as
    // wide as the bark is in the lowest tenth of the tree.
    let top = mesh.bounds().1[1];
    let up = lowest.min(0.5 * top).max(0.2 * top);
    let low: Vec<f32> = bark
        .iter()
        .map(|&i| mesh.positions[i])
        .filter(|p| p[1] < 0.1 * top)
        .map(|p| (p[0] * p[0] + p[2] * p[2]).sqrt())
        .collect();
    let base_r = (low.iter().sum::<f32>() / low.len().max(1) as f32).max(0.05);
    prism(&mut part, base_r, 0.6 * base_r, up);
    let leaf_fraction = SeasonState::new(params, season).leaf_fraction;
    let density = ((1.0 - (-0.6 * params.leaves.density).exp()) * (0.3 + 0.7 * leaf_fraction))
        .clamp(DENSITY.0, DENSITY.1);
    part.uvs.iter_mut().for_each(|uv| *uv = [density, 0.0]);
    Some(ShadowCaster { part, density })
}

/// An octahedron scaled to `radii` (times about 1.1: its volume is a third of the ellipsoid's;
/// many small ones keep the crown's area) round `centre`, appended to `part`.
fn octahedron(part: &mut MeshPart, centre: P, radii: P, wind: [f32; 2], blob: u32) {
    const V: [P; 6] = [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
    ];
    #[rustfmt::skip]
    const F: [u32; 24] = [
        0, 2, 4, 4, 2, 1, 1, 2, 5, 5, 2, 0,
        0, 4, 3, 4, 1, 3, 1, 5, 3, 5, 0, 3,
    ];
    let first = part.positions.len() as u32;
    for (k, q) in V.into_iter().enumerate() {
        // Each tip 0.8 to 1.2 times as far out (a hash of the blob and the tip), so the
        // shadow's outline is broken and leafy, not a row of identical diamonds.
        let h = ((blob * 6 + k as u32).wrapping_mul(2_654_435_761) >> 16) & 0xff;
        let f = 1.1 * (0.8 + 0.4 * h as f32 / 255.0);
        part.positions.push([
            centre[0] + q[0] * radii[0] * f,
            (centre[1] + q[1] * radii[1] * f).max(0.0),
            centre[2] + q[2] * radii[2] * f,
        ]);
        part.normals.push(q);
        part.uvs.push([0.0; 2]);
        part.wind.push(wind);
    }
    part.indices.extend(F.iter().map(|i| first + i));
}

/// A six-sided tapered prism from the ground to `height`, without caps (the crown hides its
/// top). It does not sway.
fn prism(part: &mut MeshPart, r0: f32, r1: f32, height: f32) {
    const HEX: [(f32, f32); 6] = [
        (1.0, 0.0),
        (0.5, 0.866_025_4),
        (-0.5, 0.866_025_4),
        (-1.0, 0.0),
        (-0.5, -0.866_025_4),
        (0.5, -0.866_025_4),
    ];
    let first = part.positions.len() as u32;
    for (y, r) in [(0.0, r0), (height, r1)] {
        for (c, s) in HEX {
            part.positions.push([c * r, y, s * r]);
            part.normals.push([c, 0.0, s]);
            part.uvs.push([0.0; 2]);
            part.wind.push([0.0, 0.0]);
        }
    }
    for a in 0..6u32 {
        let b = (a + 1) % 6;
        part.indices.extend([
            first + a,
            first + b,
            first + 6 + b,
            first + a,
            first + 6 + b,
            first + 6 + a,
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Species, generate_levels, species};
    use std::collections::HashMap;

    #[test]
    fn a_crown_becomes_a_few_closed_blobs_inside_its_envelope() {
        let mut checked = 0;
        for s in species().iter().filter(|s| s.params.height_m >= 5.0) {
            let p = &s.params;
            let far = generate_levels(p, 1, None, false).pop().unwrap();
            let Some(c) = shadow_caster(p, &far, None) else {
                continue;
            };
            checked += 1;
            assert!(c.part.indices.len() / 3 <= MAX_TRIANGLES, "{}", s.id);
            // blobs are closed: every edge of an octahedron is shared by two faces
            let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
            for t in c.part.indices[..BLOBS * 24].chunks(3) {
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    *edges.entry((a.min(b), a.max(b))).or_default() += 1;
                }
            }
            assert!(edges.values().all(|&n| n == 2), "{}: open blob", s.id);
            // and stay near the crown's own bounds
            let (lo, hi) = far.bounds();
            for k in 0..3 {
                let size = (hi[k] - lo[k]).max(1.0);
                let (min, max) = c
                    .part
                    .positions
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q[k]), b.max(q[k])));
                assert!(
                    min >= lo[k] - 0.4 * size && max <= hi[k] + 0.4 * size,
                    "{} axis {k}",
                    s.id
                );
            }
            assert!((DENSITY.0..=DENSITY.1).contains(&c.density));
        }
        assert!(checked > 5);
    }

    #[test]
    fn it_is_deterministic_and_denser_crowns_cast_darker() {
        let p = &Species::by_id("fagus-sylvatica").unwrap().params;
        let far = generate_levels(p, 1, None, false).pop().unwrap();
        let a = shadow_caster(p, &far, None).unwrap();
        assert_eq!(a.part, shadow_caster(p, &far, None).unwrap().part);
        let mut sparse = p.clone();
        let mut dense = p.clone();
        sparse.leaves.density = 0.3;
        dense.leaves.density = 3.0;
        let ds = shadow_caster(&sparse, &far, None).unwrap().density;
        let dd = shadow_caster(&dense, &far, None).unwrap().density;
        assert!(dd > ds + 0.2, "{ds} {dd}");
    }
}
