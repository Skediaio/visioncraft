//! How much of a planted area its crowns close from above: the fraction of the area under at
//! least one crown, counted on a grid. Headless; `planting.canopy_cover` reports it.

use std::collections::HashMap;

/// Most grid cells one measurement visits (the cell grows to stay under it).
const MAX_CELLS: f32 = 1_000_000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cover {
    /// Grid cells whose centre is in the area, times the cell's area (m2).
    pub area_m2: f32,
    pub covered_m2: f32,
    /// `covered_m2 / area_m2`, 0 for an empty area.
    pub fraction: f32,
    /// The grid cell actually used (m).
    pub cell_m: f32,
}

/// The cover of the area `inside` (within `bounds`, `[min, max]`) by `crowns` (centre, radius),
/// sampled on a grid of `cell_m`-sized cells (larger when the area is huge).
pub fn canopy_cover(
    bounds: [[f32; 2]; 2],
    inside: &dyn Fn([f32; 2]) -> bool,
    crowns: &[([f32; 2], f32)],
    cell_m: f32,
) -> Cover {
    let size = [bounds[1][0] - bounds[0][0], bounds[1][1] - bounds[0][1]];
    if !(size[0] > 0.0 && size[1] > 0.0) {
        return Cover {
            area_m2: 0.0,
            covered_m2: 0.0,
            fraction: 0.0,
            cell_m,
        };
    }
    let cell = cell_m.max(0.05).max((size[0] * size[1] / MAX_CELLS).sqrt());
    // Crowns in buckets as wide as the widest crown, so a cell tests its neighbours only.
    let reach = crowns.iter().map(|c| c.1).fold(cell, f32::max);
    let key = |p: [f32; 2]| ((p[0] / reach).floor() as i32, (p[1] / reach).floor() as i32);
    type Crown = ([f32; 2], f32);
    let mut buckets: HashMap<(i32, i32), Vec<Crown>> = HashMap::new();
    for &(c, r) in crowns.iter().filter(|c| c.1 > 0.0) {
        buckets.entry(key(c)).or_default().push((c, r * r));
    }
    let nx = (size[0] / cell).ceil() as u32;
    let ny = (size[1] / cell).ceil() as u32;
    let (mut area, mut covered) = (0u32, 0u32);
    for j in 0..ny {
        for i in 0..nx {
            let p = [
                bounds[0][0] + (i as f32 + 0.5) * cell,
                bounds[0][1] + (j as f32 + 0.5) * cell,
            ];
            if !inside(p) {
                continue;
            }
            area += 1;
            let (kx, ky) = key(p);
            let under = (kx - 1..=kx + 1).any(|x| {
                (ky - 1..=ky + 1).any(|y| {
                    buckets.get(&(x, y)).is_some_and(|v| {
                        v.iter()
                            .any(|(c, r2)| (c[0] - p[0]).powi(2) + (c[1] - p[1]).powi(2) <= *r2)
                    })
                })
            });
            covered += u32::from(under);
        }
    }
    let unit = cell * cell;
    Cover {
        area_m2: area as f32 * unit,
        covered_m2: covered as f32 * unit,
        fraction: if area > 0 {
            covered as f32 / area as f32
        } else {
            0.0
        },
        cell_m: cell,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_crowns_count_once_and_outside_cells_not_at_all() {
        let square = [[0.0, 0.0], [20.0, 20.0]];
        let all = |_: [f32; 2]| true;
        let one = canopy_cover(square, &all, &[([10.0, 10.0], 5.0)], 0.25);
        let want = std::f32::consts::PI * 25.0 / 400.0;
        assert!((one.fraction - want).abs() < 0.01, "{}", one.fraction);
        assert!((one.area_m2 - 400.0).abs() < 0.01);
        // The same crown twice, and a second one overlapping it by half.
        let twice = canopy_cover(square, &all, &[([10.0, 10.0], 5.0); 2], 0.25);
        assert_eq!(twice.fraction, one.fraction);
        let two = canopy_cover(
            square,
            &all,
            &[([10.0, 10.0], 5.0), ([14.0, 10.0], 5.0)],
            0.25,
        );
        assert!(two.fraction > one.fraction && two.fraction < 2.0 * one.fraction);
        // Only the left half is planted: a crown on the right adds nothing.
        let left = |p: [f32; 2]| p[0] < 10.0;
        let half = canopy_cover(square, &left, &[([15.0, 10.0], 4.0)], 0.5);
        assert_eq!(half.fraction, 0.0);
        assert!((half.area_m2 - 200.0).abs() < 1.0);
        // A closed carpet of crowns covers all.
        let grid: Vec<_> = (0..5)
            .flat_map(|i| (0..5).map(move |j| ([2.0 + 4.0 * i as f32, 2.0 + 4.0 * j as f32], 3.0)))
            .collect();
        assert_eq!(canopy_cover(square, &all, &grid, 0.5).fraction, 1.0);
    }
}
