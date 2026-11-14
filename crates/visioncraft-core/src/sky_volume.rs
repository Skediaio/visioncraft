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
//!
//! Two room-scale fields ride along. `room`: the open fraction averaged over the room round a
//! sample, never through a wall (a screened diffusion over the samples that see each other), for
//! the daylight a room's surfaces pass round. `lamp_bounce`: the light of the lamps passed round
//! the room they hang in, by the integrating-sphere relation for an enclosure's interreflected
//! illuminance, E = F R / (A (1 - R)) (Sumpner, "The diffusion of light", Proceedings of the
//! Physical Society of London 12, 1892; the lumen method's indirect part), with the lamp's flux
//! `F` that meets a surface, the area `A` its rays meet and the share of them a room keeps.

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
/// The mean reflectance of a room's surfaces (pale walls and ceiling, a wooden floor): the
/// renderer's `ROOM_REFLECTANCE` (scene.wgsl).
pub const ROOM_REFLECTANCE: f32 = 0.45;
/// The room average (`room`): passes of a screened diffusion, each keeping this share of the
/// neighbours' value, so a sample's average reaches about a room's half width and no further.
const ROOM_PASSES: usize = 40;
const ROOM_SPREAD: f32 = 0.9;
/// Rays a lamp casts to find the surfaces its light meets.
const LAMP_RAYS: usize = 192;
/// How far round a lamp the samples that see it take its bounced light, metres, and the rays
/// that find them (about one per sample at that reach).
const LAMP_REACH_M: f32 = 9.0;
const LAMP_SIGHT_RAYS: usize = 4096;

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
    /// Per sample: the open fraction (`samples`' first) averaged over the room round it, never
    /// through a wall; what a room lets in of the sky.
    pub room: Vec<f32>,
    voxels: Voxels,
    /// Per sample: which face neighbours it sees, bit `2 k` along -axis k, `2 k + 1` along +.
    links: Vec<u8>,
}

impl SkyVolume {
    /// The volume as bytes, for the trip back from a worker (`from_bytes`).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        let f = |b: &mut Vec<u8>, v: f32| b.extend_from_slice(&v.to_le_bytes());
        let u = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_le_bytes());
        self.origin.iter().for_each(|&v| f(&mut b, v));
        f(&mut b, self.cell);
        self.dims.iter().for_each(|&v| u(&mut b, v));
        self.voxels.origin.iter().for_each(|&v| f(&mut b, v));
        self.voxels.dims.iter().for_each(|&v| u(&mut b, v as u32));
        self.samples.iter().flatten().for_each(|&v| f(&mut b, v));
        self.room.iter().for_each(|&v| f(&mut b, v));
        b.extend_from_slice(&self.links);
        b.extend(self.voxels.solid.iter().map(|&s| s as u8));
        b
    }

    /// The volume back from `to_bytes`; an error for bytes of the wrong length.
    pub fn from_bytes(b: &[u8]) -> Result<Self, String> {
        let mut at = 0usize;
        let mut take = |n: usize| -> Result<&[u8], String> {
            let s = b.get(at..at + n).ok_or("sky volume bytes cut short")?;
            at += n;
            Ok(s)
        };
        let floats = |s: &[u8]| -> Vec<f32> {
            s.as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect()
        };
        let head = floats(take(4 * 13)?);
        let u = |i: usize| head[i].to_bits();
        let dims = [u(4), u(5), u(6)];
        let vdims = [u(10) as i32, u(11) as i32, u(12) as i32];
        let n = dims.iter().map(|&d| d as usize).product::<usize>();
        let nv = vdims.iter().map(|&d| d.max(0) as usize).product::<usize>();
        if n == 0 || n > 4 * MAX_CELLS || nv > 1 << 28 {
            return Err("sky volume bytes with impossible sizes".into());
        }
        let samples = floats(take(16 * n)?).as_chunks::<4>().0.to_vec();
        let room = floats(take(4 * n)?);
        let links = take(n)?.to_vec();
        let solid = take(nv)?.iter().map(|&v| v != 0).collect();
        if take(1).is_ok() {
            return Err("sky volume bytes too long".into());
        }
        Ok(Self {
            origin: [head[0], head[1], head[2]],
            cell: head[3],
            dims,
            samples,
            room,
            voxels: Voxels {
                origin: [head[7], head[8], head[9]],
                dims: vdims,
                solid,
            },
            links,
        })
    }
}

/// A lamp for `SkyVolume::lamp_bounce`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lamp {
    pub position: [f32; 3],
    /// Linear RGB of luminance about 1.
    pub color: [f32; 3],
    pub lumens: f32,
    /// A spot's axis; `None` for a lamp that shines all round.
    pub direction: Option<[f32; 3]>,
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

    /// The sample index for world point `p` seen from the side `n` faces (lifted off the
    /// surface by most of a cell, as the renderer reads it), if inside.
    pub fn index(&self, p: [f32; 3], n: [f32; 3]) -> Option<usize> {
        let at = self.nearest(std::array::from_fn(|k| p[k] + n[k] * 0.7 * self.cell))?;
        Some((at[0] + self.dims[0] * (at[1] + self.dims[1] * at[2])) as usize)
    }

    /// The surface-facing visibility (`facing`) of sample `i` (from `index`).
    pub fn seen(&self, i: usize, n: [f32; 3]) -> f32 {
        let s = self.samples[i];
        (s[0] + 2.0 * (s[1] * n[0] + s[2] * n[1] + s[3] * n[2])).clamp(0.0, 1.0)
    }

    /// How far along unit `d` from `p` the first occluder lies, within `metres` and the grid.
    pub fn trace(&self, p: [f32; 3], d: [f32; 3], metres: f32) -> Option<f32> {
        self.voxels.hit(p, d, metres).map(|h| h.0)
    }

    /// Whether nothing lies between `a` and `b`.
    pub fn sees(&self, a: [f32; 3], b: [f32; 3]) -> bool {
        let d: [f32; 3] = std::array::from_fn(|k| b[k] - a[k]);
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        len < 1e-4 || self.voxels.free(a, d.map(|c| c / len), len)
    }

    /// The lamps' light passed round the rooms, per sample (illuminance, lux, linear RGB): each
    /// lamp's interreflected illuminance `F R / (A (1 - k R))` (module docs) goes to every
    /// sample within `LAMP_REACH_M` that sees it (found by rays from the lamp). `F` is its flux when its light meets something
    /// (a spot: along its axis), `A` the area its rays meet (each ray's hit distance squared over
    /// the cosine at the face it meets), `k` the share of its rays that meet something (the rest
    /// leaves through the openings and does not come back).
    pub fn lamp_bounce(&self, lamps: &[Lamp]) -> Vec<[f32; 3]> {
        let mut out = vec![[0.0f32; 3]; self.samples.len()];
        let dirs = directions(LAMP_RAYS);
        let seeing = directions(LAMP_SIGHT_RAYS);
        let mut stamp = vec![0u32; self.samples.len()];
        for (n, lamp) in lamps.iter().enumerate() {
            let p = lamp.position;
            let (mut hits, mut area) = (0usize, 0.0f32);
            for d in &dirs {
                if let Some((t, k)) = self.voxels.hit(p, *d, REACH_M) {
                    hits += 1;
                    area += t * t / d[k].abs().max(0.25);
                }
            }
            if hits == 0 || lamp.lumens <= 0.0 {
                continue;
            }
            let area = area * 4.0 * std::f32::consts::PI / LAMP_RAYS as f32;
            let kept = hits as f32 / LAMP_RAYS as f32;
            let meets = match lamp.direction {
                Some(d) => {
                    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                    let hit = self.voxels.hit(p, d.map(|c| c / len), REACH_M);
                    if hit.is_some() { 1.0 } else { 0.0 }
                }
                None => kept,
            };
            let e = lamp.lumens * meets * ROOM_REFLECTANCE
                / ((1.0 - kept * ROOM_REFLECTANCE) * area.max(1.0));
            if e < 1e-3 {
                continue;
            }
            // The samples it sees: along rays from it to what they meet (spaced finer than a
            // sample at the reach), each sample once.
            for d in &seeing {
                let end = self
                    .voxels
                    .hit(p, *d, LAMP_REACH_M)
                    .map_or(LAMP_REACH_M, |h| h.0);
                let mut t = 0.0;
                while t <= end {
                    let q: [f32; 3] = std::array::from_fn(|k| p[k] + d[k] * t);
                    let Some(at) = self.nearest(q) else { break };
                    let i = (at[0] + self.dims[0] * (at[1] + self.dims[1] * at[2])) as usize;
                    if stamp[i] != n as u32 + 1 {
                        stamp[i] = n as u32 + 1;
                        for k in 0..3 {
                            out[i][k] += e * lamp.color[k];
                        }
                    }
                    t += 0.5 * self.cell;
                }
            }
        }
        // Soften the edge of what each lamp sees.
        for _ in 0..2 {
            out = average_linked(&out, &self.links, self.dims);
        }
        out
    }
}

/// Each sample's value averaged half and half with the mean of the face neighbours it sees.
fn average_linked(values: &[[f32; 3]], links: &[u8], dims: [u32; 3]) -> Vec<[f32; 3]> {
    let strides = [1, dims[0] as usize, (dims[0] * dims[1]) as usize];
    (0..values.len())
        .map(|i| {
            let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
            for b in 0..6 {
                if links[i] & (1 << b) != 0 {
                    let j = if b % 2 == 0 {
                        i - strides[b / 2]
                    } else {
                        i + strides[b / 2]
                    };
                    (0..3).for_each(|k| sum[k] += values[j][k]);
                    n += 1.0;
                }
            }
            if n == 0.0 {
                return values[i];
            }
            std::array::from_fn(|k| 0.5 * values[i][k] + 0.5 * sum[k] / n)
        })
        .collect()
}

/// Evenly spread unit directions (a Fibonacci sphere).
fn directions(count: usize) -> Vec<[f32; 3]> {
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    (0..count)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f32 + 0.5) / count as f32;
            let r = (1.0 - y * y).sqrt();
            let a = golden * i as f32;
            [r * a.cos(), y, r * a.sin()]
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
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
        self.hit(p, d, metres).is_none()
    }

    /// The first solid voxel within `metres` along unit `d` from `p`: its distance (metres) and
    /// the axis of the face the ray enters it by.
    fn hit(&self, p: [f32; 3], d: [f32; 3], metres: f32) -> Option<(f32, usize)> {
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
            let t = next[k];
            if t > reach {
                return None;
            }
            v[k] += step[k];
            next[k] += delta[k];
            match self.index(v) {
                None => return None,
                Some(i) if self.solid[i] => return Some((t * VOXEL_M, k)),
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
    let dirs = directions(RAYS);
    // One z slab of samples (x fastest, then y).
    let slab = |z: u32| -> Vec<[f32; 4]> {
        let mut out = Vec::with_capacity((dims[0] * dims[1]) as usize);
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
                out.push(sum.map(|v| v / RAYS as f32));
            }
        }
        out
    };
    // Natively the slabs are shared out over the cores; the browser has one thread (the app
    // runs this in a worker there).
    #[cfg(not(target_arch = "wasm32"))]
    let samples: Vec<[f32; 4]> = {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()) as u32;
        let per = dims[2].div_ceil(threads.max(1));
        std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..dims[2])
                .step_by(per as usize)
                .map(|z0| {
                    let slab = &slab;
                    scope.spawn(move || {
                        (z0..(z0 + per).min(dims[2]))
                            .flat_map(slab)
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            jobs.into_iter()
                .flat_map(|j| j.join().expect("a sky volume thread panicked"))
                .collect()
        })
    };
    #[cfg(target_arch = "wasm32")]
    let samples: Vec<[f32; 4]> = (0..dims[2]).flat_map(slab).collect();
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
    // Which face neighbours each sample sees, then the room average of the open fraction: a
    // screened diffusion over those links, normalised by the same sums over ones.
    let mut links = vec![0u8; smooth.len()];
    for z in 0..dims[2] as i64 {
        for y in 0..dims[1] as i64 {
            for x in 0..dims[0] as i64 {
                let p = [x, y, z].map(|v| v as f32);
                let p: [f32; 3] = std::array::from_fn(|k| lo[k] + cell * p[k]);
                let i = at(x, y, z).expect("in the grid");
                for b in 0..6 {
                    let mut o = [0i64; 3];
                    o[b / 2] = if b % 2 == 0 { -1 } else { 1 };
                    if at(x + o[0], y + o[1], z + o[2]).is_some()
                        && voxels.free(p, o.map(|c| c as f32), cell)
                    {
                        links[i] |= 1 << b;
                    }
                }
            }
        }
    }
    let open: Vec<f32> = smooth.iter().map(|s| s[0]).collect();
    let strides = [1, dims[0] as usize, (dims[0] * dims[1]) as usize];
    let mut sums: Vec<[f32; 2]> = open.iter().map(|&a| [a, 1.0]).collect();
    for _ in 0..ROOM_PASSES {
        sums = (0..sums.len())
            .map(|i| {
                let (mut s, mut n) = ([0.0f32; 2], 0.0f32);
                for b in 0..6 {
                    if links[i] & (1 << b) != 0 {
                        let j = if b % 2 == 0 {
                            i - strides[b / 2]
                        } else {
                            i + strides[b / 2]
                        };
                        s[0] += sums[j][0];
                        s[1] += sums[j][1];
                        n += 1.0;
                    }
                }
                let n = n.max(1.0);
                [
                    open[i] + ROOM_SPREAD * s[0] / n,
                    1.0 + ROOM_SPREAD * s[1] / n,
                ]
            })
            .collect();
    }
    let room = sums.iter().map(|s| (s[0] / s[1]).clamp(0.0, 1.0)).collect();
    Some(SkyVolume {
        origin: lo,
        cell,
        dims,
        samples: smooth,
        room,
        voxels,
        links,
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
    fn a_lamp_in_a_closed_room_lights_it_as_an_integrating_sphere_says() {
        let v = sky_volume(&room(0.0)).unwrap();
        let lamp = Lamp {
            position: [3.0, 2.0, 3.0],
            color: [1.0, 0.5, 0.25],
            lumens: 1000.0,
            direction: None,
        };
        let e = v.lamp_bounce(&[lamp]);
        // 6 x 3 x 6 m: 144 m2 of surfaces, all of the light kept.
        let expected = 1000.0 * ROOM_REFLECTANCE / ((1.0 - ROOM_REFLECTANCE) * 144.0);
        let inside = e[v.index([1.5, 1.0, 4.5], [0.0; 3]).unwrap()];
        assert!(
            (inside[0] / expected - 1.0).abs() < 0.25,
            "{inside:?} {expected}"
        );
        assert!((inside[1] / inside[0] - 0.5).abs() < 1e-4);
        // Next to none over the roof (only the softened edge of the samples in the roof).
        let above = e[v.index([3.0, 3.6, 3.0], [0.0; 3]).unwrap()];
        assert!(above[0] < 0.05 * inside[0], "{above:?}");
        // The room's own daylight: none in a closed room, some by an opening, less deeper in.
        assert!(v.room[v.index([3.0, 1.5, 3.0], [0.0; 3]).unwrap()] < 0.01);
        let v = sky_volume(&room(2.0)).unwrap();
        let near = v.room[v.index([5.2, 1.5, 3.0], [0.0; 3]).unwrap()];
        let deep = v.room[v.index([0.8, 1.5, 3.0], [0.0; 3]).unwrap()];
        assert!(near > deep && deep > 0.0, "{near} {deep}");
    }

    #[test]
    fn a_volume_survives_the_trip_through_bytes() {
        let v = sky_volume(&room(2.0)).unwrap();
        let bytes = v.to_bytes();
        assert_eq!(SkyVolume::from_bytes(&bytes).unwrap(), v);
        assert!(SkyVolume::from_bytes(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn nothing_to_occlude_is_no_volume() {
        assert!(sky_volume(&[]).is_none());
    }
}
