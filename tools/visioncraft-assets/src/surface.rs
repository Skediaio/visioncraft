//! How far apart two textured surfaces are: the share of one surface that has no face of the
//! other near it that faces the same way and shows the same spot of the same texture.
//! Simplification checks use it both ways: faces spanned across empty space, turned round or
//! with smeared texture coordinates stray from the original, holes leave some of it uncovered.

use crate::scene::Model;
use std::collections::HashMap;

/// Points sampled per surface.
const SAMPLES: usize = 4096;

type V = [f64; 3];

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

struct Tri {
    p: [V; 3],
    uv: [[f64; 2]; 3],
    /// Unnormalised face normal.
    n: V,
    mat: usize,
}

impl Tri {
    /// The point and texture coordinate at barycentric `(u, v)` (weights of corners 1 and 2).
    fn at(&self, u: f64, v: f64) -> (V, [f64; 2]) {
        let w = 1.0 - u - v;
        (
            std::array::from_fn(|k| w * self.p[0][k] + u * self.p[1][k] + v * self.p[2][k]),
            std::array::from_fn(|k| w * self.uv[0][k] + u * self.uv[1][k] + v * self.uv[2][k]),
        )
    }

    /// Barycentric `(u, v)` of the point of the triangle closest to `q` (Ericson, Real-Time
    /// Collision Detection, 5.1.5).
    fn closest(&self, q: V) -> (f64, f64) {
        let [a, b, c] = self.p;
        let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(q, a));
        let (d1, d2) = (dot(ab, ap), dot(ac, ap));
        if d1 <= 0.0 && d2 <= 0.0 {
            return (0.0, 0.0);
        }
        let bp = sub(q, b);
        let (d3, d4) = (dot(ab, bp), dot(ac, bp));
        if d3 >= 0.0 && d4 <= d3 {
            return (1.0, 0.0);
        }
        let cp = sub(q, c);
        let (d5, d6) = (dot(ab, cp), dot(ac, cp));
        if d6 >= 0.0 && d5 <= d6 {
            return (0.0, 1.0);
        }
        let (vc, vb, va) = (d1 * d4 - d3 * d2, d5 * d2 - d1 * d6, d3 * d6 - d5 * d4);
        if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
            (d1 / (d1 - d3), 0.0)
        } else if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
            (0.0, d2 / (d2 - d6))
        } else if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
            let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
            (1.0 - w, w)
        } else {
            let s = 1.0 / (va + vb + vc);
            (vb * s, vc * s)
        }
    }
}

/// Every non-degenerate triangle.
fn triangles(m: &Model) -> Vec<Tri> {
    let mut out = Vec::new();
    for mesh in &m.meshes {
        for t in mesh.idx.as_chunks::<3>().0 {
            let p = t.map(|i| mesh.pos[i as usize].map(f64::from));
            let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            if dot(n, n) > 0.0 {
                let uv = t.map(|i| mesh.uv[i as usize].map(f64::from));
                out.push(Tri {
                    p,
                    uv,
                    n,
                    mat: mesh.mat,
                });
            }
        }
    }
    out
}

/// Triangles bucketed by the grid cells their boxes touch.
struct Grid {
    cell: f64,
    cells: HashMap<[i64; 3], Vec<u32>>,
    tris: Vec<Tri>,
}

impl Grid {
    fn new(tris: Vec<Tri>, cell: f64) -> Self {
        let mut cells: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
        for (i, t) in tris.iter().enumerate() {
            let lo: [i64; 3] = std::array::from_fn(|k| {
                (t.p.iter().map(|p| p[k]).fold(f64::MAX, f64::min) / cell).floor() as i64
            });
            let hi: [i64; 3] = std::array::from_fn(|k| {
                (t.p.iter().map(|p| p[k]).fold(f64::MIN, f64::max) / cell).floor() as i64
            });
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        cells.entry([x, y, z]).or_default().push(i as u32);
                    }
                }
            }
        }
        Self { cell, cells, tris }
    }

    /// Is there a triangle of material `mat` within `d` (at most the cell size) of `p`, facing
    /// within 90 degrees of `n`, whose texture coordinate at the closest point is within
    /// `uv_tol` of `uv`?
    fn near(&self, p: V, n: V, uv: [f64; 2], mat: usize, d: f64, uv_tol: f64) -> bool {
        let c: [i64; 3] = std::array::from_fn(|k| (p[k] / self.cell).floor() as i64);
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    let Some(list) = self.cells.get(&[c[0] + x, c[1] + y, c[2] + z]) else {
                        continue;
                    };
                    for &i in list {
                        let t = &self.tris[i as usize];
                        if t.mat != mat || dot(t.n, n) <= 0.0 {
                            continue;
                        }
                        let (u, v) = t.closest(p);
                        let (q, quv) = t.at(u, v);
                        let e = sub(p, q);
                        let (du, dv) = (quv[0] - uv[0], quv[1] - uv[1]);
                        if dot(e, e) <= d * d && du * du + dv * dv <= uv_tol * uv_tol {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
}

/// Share of the area of `from` with no face of `to` within `d` that has the same material,
/// faces the same way (within 90 degrees) and has texture coordinates within `uv_tol` at the
/// closest point. Sampled at [`SAMPLES`] points spread evenly over the area.
pub fn stray_share(from: &Model, to: &Model, d: f32, uv_tol: f32) -> f64 {
    let src = triangles(from);
    let mut acc = Vec::with_capacity(src.len());
    let mut total = 0.0;
    for t in &src {
        total += dot(t.n, t.n).sqrt();
        acc.push(total);
    }
    if total <= 0.0 {
        return 0.0;
    }
    let largest = f64::from(to.size().into_iter().fold(0.0_f32, f32::max));
    let d = f64::from(d);
    let grid = Grid::new(triangles(to), d.max(largest / 32.0).max(1e-9));
    let mut stray = 0;
    for s in 0..SAMPLES {
        let at = (s as f64 + 0.5) / SAMPLES as f64 * total;
        let t = &src[acc.partition_point(|&a| a < at).min(src.len() - 1)];
        // R2 low-discrepancy sequence folded into the triangle
        let (mut u, mut v) = (
            (0.5 + s as f64 * 0.754_877_666_246_692_7).fract(),
            (0.5 + s as f64 * 0.569_840_290_998_053_3).fract(),
        );
        if u + v > 1.0 {
            (u, v) = (1.0 - u, 1.0 - v);
        }
        let (p, uv) = t.at(u, v);
        if !grid.near(p, t.n, uv, t.mat, d, f64::from(uv_tol)) {
            stray += 1;
        }
    }
    stray as f64 / SAMPLES as f64
}
