//! How much of the sky each point round the buildings sees: a coarse grid of directional sky
//! visibility the renderer samples for the ambient light, so a room under a roof does not get
//! the open sky's light on its floor and a windowless corridor goes dim.
//!
//! The technique is an irradiance volume (Greger, Shirley, Hubbard and Greenberg, "The
//! Irradiance Volume", IEEE Computer Graphics and Applications 18(2), 1998): a regular grid of
//! samples, interpolated trilinearly. Each sample stores the visibility of the sky as order-1
//! spherical harmonics (Sloan, "Stupid Spherical Harmonics (SH) Tricks", GDC 2008), found by
//! marching rays through a voxelisation of the occluders. It is computed on the CPU once per
//! change of geometry, cheap enough for the browser.

/// The occluders' voxel size, metres. Thinner than a wall, so a wall is at least one voxel.
const VOXEL_M: f32 = 0.2;
/// Sample spacing, metres (two voxels); grows for large sites to stay within `MAX_CELLS`.
const CELL_M: f32 = 0.4;
const MAX_CELLS: usize = 160_000;
/// Rays per sample. Each sample turns the set by its own angle, so the steps through a window
/// become noise the interpolation smooths instead of bands.
const RAYS: usize = 48;
/// A ray that travels this far without a hit sees the sky (beyond a room's reach).
const REACH_M: f32 = 14.0;
/// Room round the occluders, metres.
const PAD_M: f32 = 1.0;

/// The grid: sample `(x, y, z)` sits at `origin + cell * (x, y, z)` and holds
/// `(a0, ax, ay, az)`: the visible fraction of all directions and the mean visible direction
/// (the sky's visibility as L1 SH, without the basis constants). The cosine-weighted visibility
/// of a surface facing `n` is `clamp(a0 + 2 a.n, 0, 1)`: 1 under the open sky, 1 facing away
/// from a lone wall, 0 in a closed room.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyVolume {
    pub origin: [f32; 3],
    pub cell: f32,
    pub dims: [u32; 3],
    /// x fastest, then y, then z.
    pub samples: Vec<[f32; 4]>,
}

impl SkyVolume {
    /// The surface-facing visibility at sample `(x, y, z)` (tests and tools).
    pub fn facing(&self, at: [u32; 3], n: [f32; 3]) -> f32 {
        let [d0, d1, _] = self.dims;
        let s = self.samples[(at[0] + d0 * (at[1] + d1 * at[2])) as usize];
        (s[0] + 2.0 * (s[1] * n[0] + s[2] * n[1] + s[3] * n[2])).clamp(0.0, 1.0)
    }

    /// The sample nearest to world point `p`.
    pub fn nearest(&self, p: [f32; 3]) -> Option<[u32; 3]> {
        let mut at = [0; 3];
        for k in 0..3 {
            let i = ((p[k] - self.origin[k]) / self.cell).round();
            if i < 0.0 || i >= self.dims[k] as f32 {
                return None;
            }
            at[k] = i as u32;
        }
        Some(at)
    }
}

/// Evenly spread unit directions (a Fibonacci sphere).
fn directions() -> Vec<[f32; 3]> {
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    (0..RAYS)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f32 + 0.5) / RAYS as f32;
            let r = (1.0 - y * y).sqrt();
            let a = golden * i as f32;
            [r * a.cos(), y, r * a.sin()]
        })
        .collect()
}

struct Voxels {
    origin: [f32; 3],
    dims: [i32; 3],
    solid: Vec<bool>,
}

impl Voxels {
    fn index(&self, v: [i32; 3]) -> Option<usize> {
        (0..3)
            .all(|k| (0..self.dims[k]).contains(&v[k]))
            .then(|| (v[0] + self.dims[0] * (v[1] + self.dims[1] * v[2])) as usize)
    }

    /// Whether a ray from `p` along unit `d` leaves without a hit (Amanatides and Woo, "A Fast
    /// Voxel Traversal Algorithm for Ray Tracing", Eurographics 1987).
    fn escapes(&self, p: [f32; 3], d: [f32; 3]) -> bool {
        self.free(p, d, REACH_M)
    }

    /// Whether `metres` along unit `d` from `p` hit nothing (leaving the grid counts as free).
    fn free(&self, p: [f32; 3], d: [f32; 3], metres: f32) -> bool {
        let local: [f32; 3] = std::array::from_fn(|k| (p[k] - self.origin[k]) / VOXEL_M);
        let mut v: [i32; 3] = std::array::from_fn(|k| local[k].floor() as i32);
        let step: [i32; 3] = std::array::from_fn(|k| if d[k] >= 0.0 { 1 } else { -1 });
        let delta: [f32; 3] = std::array::from_fn(|k| (1.0 / d[k]).abs());
        let mut next: [f32; 3] = std::array::from_fn(|k| {
            let edge = if d[k] >= 0.0 {
                local[k].floor() + 1.0 - local[k]
            } else {
                local[k] - local[k].floor()
            };
            edge * delta[k]
        });
        let reach = metres / VOXEL_M;
        loop {
            let k = if next[0] < next[1] {
                if next[0] < next[2] { 0 } else { 2 }
            } else if next[1] < next[2] {
                1
            } else {
                2
            };
            if next[k] > reach {
                return true;
            }
            v[k] += step[k];
            next[k] += delta[k];
            match self.index(v) {
                None => return true,
                Some(i) if self.solid[i] => return false,
                Some(_) => {}
            }
        }
    }
}

/// The sky visibility round `triangles` (world space; the surfaces that block the sky:
/// walls, roofs, floors, not glass). `None` without triangles.
pub fn sky_volume(triangles: &[[[f32; 3]; 3]]) -> Option<SkyVolume> {
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for t in triangles {
        for p in t {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    if !lo[0].is_finite() {
        return None;
    }
    let lo: [f32; 3] = std::array::from_fn(|k| lo[k] - PAD_M);
    let hi: [f32; 3] = std::array::from_fn(|k| hi[k] + PAD_M);
    let mut cell = CELL_M;
    let count =
        |c: f32| -> [u32; 3] { std::array::from_fn(|k| ((hi[k] - lo[k]) / c).ceil() as u32 + 1) };
    while count(cell).iter().map(|&n| n as usize).product::<usize>() > MAX_CELLS {
        cell *= 1.25;
    }
    let dims = count(cell);
    // The voxels: every triangle sampled finer than a voxel.
    let vdims: [i32; 3] = std::array::from_fn(|k| ((hi[k] - lo[k]) / VOXEL_M).ceil() as i32 + 1);
    let mut voxels = Voxels {
        origin: lo,
        dims: vdims,
        solid: vec![false; vdims.iter().map(|&n| n as usize).product()],
    };
    let len = |a: [f32; 3], b: [f32; 3]| (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt();
    for [a, b, c] in triangles {
        let n = ((len(*a, *b).max(len(*a, *c)).max(len(*b, *c)) / (0.5 * VOXEL_M)).ceil() as usize)
            .max(1);
        for i in 0..=n {
            for j in 0..=n - i {
                let (u, w) = (i as f32 / n as f32, j as f32 / n as f32);
                let v: [i32; 3] = std::array::from_fn(|k| {
                    let p = a[k] + (b[k] - a[k]) * u + (c[k] - a[k]) * w;
                    ((p - lo[k]) / VOXEL_M).floor() as i32
                });
                if let Some(i) = voxels.index(v) {
                    voxels.solid[i] = true;
                }
            }
        }
    }
    let dirs = directions();
    let mut samples = Vec::with_capacity(dims.iter().map(|&n| n as usize).product());
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let p = [
                    lo[0] + cell * x as f32,
                    lo[1] + cell * y as f32,
                    lo[2] + cell * z as f32,
                ];
                // Its own turn of the ray set about the vertical.
                let h = (x.wrapping_mul(73_856_093)
                    ^ y.wrapping_mul(19_349_663)
                    ^ z.wrapping_mul(83_492_791))
                    % 1024;
                let (s, c) = (h as f32 * std::f32::consts::TAU / 1024.0).sin_cos();
                let mut sum = [0.0f32; 4];
                for d in &dirs {
                    let d = [c * d[0] - s * d[2], d[1], s * d[0] + c * d[2]];
                    if voxels.escapes(p, d) {
                        sum[0] += 1.0;
                        sum[1] += d[0];
                        sum[2] += d[1];
                        sum[3] += d[2];
                    }
                }
                samples.push(sum.map(|v| v / RAYS as f32));
            }
        }
    }
    // Each sample averaged with the neighbours it sees (3 x 3 x 3, never through a wall): the
    // ray count's noise goes, the rooms stay apart.
    let at = |x: i64, y: i64, z: i64| -> Option<usize> {
        let ok = x >= 0
            && y >= 0
            && z >= 0
            && x < dims[0] as i64
            && y < dims[1] as i64
            && z < dims[2] as i64;
        ok.then(|| (x + dims[0] as i64 * (y + dims[1] as i64 * z)) as usize)
    };
    let mut smooth = samples.clone();
    for z in 0..dims[2] as i64 {
        for y in 0..dims[1] as i64 {
            for x in 0..dims[0] as i64 {
                let p = [
                    lo[0] + cell * x as f32,
                    lo[1] + cell * y as f32,
                    lo[2] + cell * z as f32,
                ];
                let (mut sum, mut n) = ([0.0f32; 4], 0.0);
                for (dx, dy, dz) in (0..27).map(|k| (k % 3 - 1, k / 3 % 3 - 1, k / 9 - 1)) {
                    let Some(i) = at(x + dx, y + dy, z + dz) else {
                        continue;
                    };
                    let d = [dx as f32, dy as f32, dz as f32];
                    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    if len > 0.0 && !voxels.free(p, d.map(|c| c / len), len * cell) {
                        continue;
                    }
                    for c in 0..4 {
                        sum[c] += samples[i][c];
                    }
                    n += 1.0;
                }
                smooth[at(x, y, z).expect("in the grid")] = sum.map(|v| v / n);
            }
        }
    }
    Some(SkyVolume {
        origin: lo,
        cell,
        dims,
        samples: smooth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two triangles of an axis-aligned rectangle `lo..hi` (one axis flat).
    fn quad(lo: [f32; 3], hi: [f32; 3]) -> Vec<[[f32; 3]; 3]> {
        let flat = (0..3).find(|&k| lo[k] == hi[k]).unwrap();
        let (u, v) = ((flat + 1) % 3, (flat + 2) % 3);
        let corner = |a: bool, b: bool| {
            let mut p = lo;
            p[u] = if a { hi[u] } else { lo[u] };
            p[v] = if b { hi[v] } else { lo[v] };
            p
        };
        vec![
            [
                corner(false, false),
                corner(true, false),
                corner(true, true),
            ],
            [
                corner(false, false),
                corner(true, true),
                corner(false, true),
            ],
        ]
    }

    /// A 6 x 3 x 6 m room, closed but for a hole of `door` metres in its east wall.
    fn room(door: f32) -> Vec<[[f32; 3]; 3]> {
        let mut t = Vec::new();
        t.extend(quad([0.0, 0.0, 0.0], [6.0, 0.0, 6.0]));
        t.extend(quad([0.0, 3.0, 0.0], [6.0, 3.0, 6.0]));
        t.extend(quad([0.0, 0.0, 0.0], [0.0, 3.0, 6.0]));
        t.extend(quad([0.0, 0.0, 0.0], [6.0, 3.0, 0.0]));
        t.extend(quad([0.0, 0.0, 6.0], [6.0, 3.0, 6.0]));
        t.extend(quad([6.0, 0.0, 0.0], [6.0, 3.0, 3.0 - door / 2.0]));
        t.extend(quad([6.0, 0.0, 3.0 + door / 2.0], [6.0, 3.0, 6.0]));
        t
    }

    #[test]
    fn a_closed_room_sees_no_sky_and_the_open_air_all_of_it() {
        let v = sky_volume(&room(0.0)).unwrap();
        let inside = v.nearest([3.0, 1.5, 3.0]).unwrap();
        assert_eq!(v.facing(inside, [0.0, 1.0, 0.0]), 0.0);
        // Over the roof: a surface facing up sees the whole sky.
        let above = v.nearest([3.0, 3.6, 3.0]).unwrap();
        assert!(v.facing(above, [0.0, 1.0, 0.0]) > 0.95, "{v:?}");
        // Beside the west wall, facing away from it.
        let beside = v.nearest([-0.6, 1.5, 3.0]).unwrap();
        assert!(v.facing(beside, [-1.0, 0.0, 0.0]) > 0.85);
    }

    #[test]
    fn an_opening_lets_in_sky_from_its_side_only() {
        let v = sky_volume(&room(2.0)).unwrap();
        let near = v.nearest([5.2, 1.5, 3.0]).unwrap();
        let (towards, away) = (
            v.facing(near, [-1.0, 0.0, 0.0]),
            v.facing(near, [1.0, 0.0, 0.0]),
        );
        // A wall facing the opening sees it; one with its back to it does not.
        assert!(away > 0.15 && towards < 0.05, "{away} {towards}");
        let deep = v.nearest([0.8, 1.5, 3.0]).unwrap();
        let far = v.facing(deep, [1.0, 0.0, 0.0]);
        assert!(far > 0.0 && far < away, "{far} {away}");
    }

    #[test]
    fn nothing_to_occlude_is_no_volume() {
        assert!(sky_volume(&[]).is_none());
    }
}
