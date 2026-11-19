//! Which of a crown's ellipsoids (its lobes and foliage masses, `gen_tree`) a point can be
//! nearest to, or inside: a grid over them lists per cell the few that can, so a query tests a
//! handful instead of all ninety. The lists come from real-number bounds of each ellipsoid's
//! distance over the cell with a wide margin for rounding, and the callers test the listed ones
//! in their order with the same arithmetic as a full scan: the answer is the full scan's, bit
//! for bit (points outside the grid scan everything).

use crate::math::V3;

/// Cells per axis.
const CELLS: usize = 16;
/// Relative slack on the bounds: the float distances the callers compute are within about
/// 1e-6 of the real ones (a handful of correctly rounded operations on non-negative terms).
const SLACK: f64 = 1e-4;

pub(crate) struct NearIndex {
    lo: V3,
    inv: V3,
    /// Offsets into `list`, one per cell and one past the end.
    start: Vec<u32>,
    list: Vec<u16>,
}

/// One axis of the real squared normalised distance `sum ((p - c) / r)^2` of an ellipsoid
/// (centre `c`, radius `r` on this axis) from the points of the interval `[a, b]`: its least
/// and its largest; `None` for a degenerate radius.
fn bounds(c: f32, r: f32, a: f64, b: f64) -> Option<(f64, f64)> {
    let (c, r2) = (f64::from(c), f64::from(r) * f64::from(r));
    if !(r2 > 0.0 && r2.is_finite() && c.is_finite()) {
        return None;
    }
    let (da, db) = ((a - c).abs(), (b - c).abs());
    let near = if (a..=b).contains(&c) {
        0.0
    } else {
        da.min(db)
    };
    let far = da.max(db);
    Some((near * near / r2, far * far / r2))
}

impl NearIndex {
    /// The ellipsoids (`(centre, radii)`) that can be the nearest to a point of each cell of
    /// the box `[lo, hi]`, by the distance `bounds` measures.
    pub fn nearest(ells: &[(V3, V3)], lo: V3, hi: V3) -> NearIndex {
        Self::build(ells, lo, hi, |b| {
            let t = b.iter().map(|x| x.1).fold(f64::MAX, f64::min);
            t * (1.0 + SLACK) + 1e-12
        })
    }

    /// The ellipsoids, each enlarged by up to `wmax` times, that can contain a point of each
    /// cell of the box `[lo, hi]`.
    pub fn containing(ells: &[(V3, V3)], lo: V3, hi: V3, wmax: f64) -> NearIndex {
        let t = wmax * wmax * (1.0 + SLACK) + 1e-12;
        Self::build(ells, lo, hi, |_| t)
    }

    fn build(
        ells: &[(V3, V3)],
        lo: V3,
        hi: V3,
        // The largest least distance an ellipsoid of a cell may have and be listed, from the
        // cell's bounds of them all.
        threshold: impl Fn(&[(f64, f64)]) -> f64,
    ) -> NearIndex {
        let size = |a: f32, b: f32| ((b - a) / CELLS as f32).max(1e-3);
        let size = [size(lo.x, hi.x), size(lo.y, hi.y), size(lo.z, hi.z)];
        let lo3 = [lo.x, lo.y, lo.z];
        let ne = ells.len();
        // The distance is a sum over the axes: each axis' bounds per cell row, once. A cell is
        // grown by a tenth of itself each way, so a point whose float key rounds into a
        // neighbouring cell is still inside it. A degenerate ellipsoid is always tested.
        let mut axes = [const { Vec::new() }; 3];
        let mut degenerate = vec![false; ne];
        for (k, axis) in axes.iter_mut().enumerate() {
            let s = f64::from(size[k]);
            for i in 0..CELLS {
                let a = f64::from(lo3[k]) + (i as f64 - 0.1) * s;
                let b = f64::from(lo3[k]) + (i as f64 + 1.1) * s;
                for (e, &(c, r)) in ells.iter().enumerate() {
                    let (c, r) = ([c.x, c.y, c.z][k], [r.x, r.y, r.z][k]);
                    axis.push(bounds(c, r, a, b).unwrap_or_else(|| {
                        degenerate[e] = true;
                        (0.0, 0.0)
                    }));
                }
            }
        }
        let mut start = Vec::with_capacity(CELLS * CELLS * CELLS + 1);
        let mut list = Vec::new();
        let mut b = vec![(0.0, 0.0); ne];
        for y in 0..CELLS {
            for z in 0..CELLS {
                for x in 0..CELLS {
                    start.push(list.len() as u32);
                    let (bx, by, bz) = (
                        &axes[0][x * ne..(x + 1) * ne],
                        &axes[1][y * ne..(y + 1) * ne],
                        &axes[2][z * ne..(z + 1) * ne],
                    );
                    for e in 0..ne {
                        b[e] = if degenerate[e] {
                            (0.0, f64::INFINITY)
                        } else {
                            (bx[e].0 + by[e].0 + bz[e].0, bx[e].1 + by[e].1 + bz[e].1)
                        };
                    }
                    let t = threshold(&b);
                    list.extend((0..ne).filter(|&i| b[i].0 <= t).map(|i| i as u16));
                }
            }
        }
        start.push(list.len() as u32);
        NearIndex {
            lo,
            inv: crate::math::v3(1.0 / size[0], 1.0 / size[1], 1.0 / size[2]),
            start,
            list,
        }
    }

    /// The ellipsoids to test for `p`, in their order; `None` outside the grid (test all).
    pub fn at(&self, p: V3) -> Option<&[u16]> {
        let key = |v: f32, lo: f32, inv: f32| {
            let k = crate::math::floor(f64::from((v - lo) * inv));
            (k >= 0.0 && k < CELLS as f64).then_some(k as usize)
        };
        let x = key(p.x, self.lo.x, self.inv.x)?;
        let y = key(p.y, self.lo.y, self.inv.y)?;
        let z = key(p.z, self.lo.z, self.inv.z)?;
        let c = (y * CELLS + z) * CELLS + x;
        Some(&self.list[self.start[c] as usize..self.start[c + 1] as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::NearIndex;
    use crate::math::{Rng, V3, v3};

    fn random_ellipsoids(r: &mut Rng, n: usize) -> Vec<(V3, V3)> {
        (0..n)
            .map(|_| {
                (
                    v3(r.sym() * 6.0, 2.0 + r.f() * 8.0, r.sym() * 6.0),
                    v3(r.range(0.3, 3.0), r.range(0.3, 3.0), r.range(0.3, 3.0)),
                )
            })
            .collect()
    }

    fn d(p: V3, (c, r): (V3, V3)) -> f32 {
        let q = p - c;
        (q.x / r.x).powi(2) + (q.y / r.y).powi(2) + (q.z / r.z).powi(2)
    }

    /// The first nearest by a full scan and by the index's lists are the same ellipsoid, for
    /// points inside the grid, on its cell borders and outside it.
    #[test]
    fn the_index_finds_what_a_full_scan_finds() {
        let mut r = Rng::new(5, 6);
        for _ in 0..20 {
            let ells = random_ellipsoids(&mut r, 90);
            let (lo, hi) = (v3(-8.0, 0.0, -8.0), v3(8.0, 12.0, 8.0));
            let idx = NearIndex::nearest(&ells, lo, hi);
            for i in 0..4000 {
                let p = if i % 4 == 0 {
                    // On a cell border.
                    v3(
                        -8.0 + (i % 17) as f32,
                        (i % 13) as f32 * 0.75,
                        r.sym() * 9.0,
                    )
                } else {
                    v3(r.sym() * 10.0, r.f() * 14.0 - 1.0, r.sym() * 10.0)
                };
                let full = (0..ells.len()).min_by(|&a, &b| d(p, ells[a]).total_cmp(&d(p, ells[b])));
                let listed = idx.at(p).map(|l| {
                    l.iter()
                        .map(|&k| k as usize)
                        .min_by(|&a, &b| d(p, ells[a]).total_cmp(&d(p, ells[b])))
                });
                if let Some(listed) = listed {
                    assert_eq!(listed, full, "{p:?}");
                }
            }
        }
    }

    /// Every ellipsoid that holds a point (at any enlargement up to the most) is listed there.
    #[test]
    fn the_index_lists_every_ellipsoid_holding_a_point() {
        let mut r = Rng::new(7, 8);
        let ells = random_ellipsoids(&mut r, 90);
        let (lo, hi) = (v3(-8.0, 0.0, -8.0), v3(8.0, 12.0, 8.0));
        let idx = NearIndex::containing(&ells, lo, hi, 1.4);
        for _ in 0..20_000 {
            let p = v3(r.sym() * 8.0, r.f() * 12.0, r.sym() * 8.0);
            let w = r.range(0.6, 1.4);
            let list = idx.at(p).unwrap_or(&[]);
            for (k, &(c, rr)) in ells.iter().enumerate() {
                if d(p, (c, rr * w)) <= 1.0 {
                    assert!(list.contains(&(k as u16)), "{p:?} in {k}");
                }
            }
        }
    }
}
