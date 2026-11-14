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
//! Light passed round the rooms rides along, gathered over the same rays: each ray that meets a
//! face (rather than the sky, or ground under the open sky) keeps which face, and a sample's
//! bounced light is the mean over its rays of what the faces they meet receive, reflected, a few
//! times over (Jacobi iterations of the radiosity equation gathered along fixed rays: Cohen and
//! Greenberg, "The Hemi-Cube", SIGGRAPH 1985, with the faces' light read from the volume). `room`
//! is the sky's light passed round so; `sun_bounce` the sun's, from the faces its parallel rays
//! reach (the sun patch on a floor), when the sun moves. `lamp_bounce`: the light of the lamps passed round
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
/// The thinnest gap a sample stands for air in (`validity`), metres.
const MIN_GAP_M: f32 = 0.3;
/// Bounces gathered for the light passed round a room (`gather`); R^4 of it is left out.
const BOUNCES: usize = 3;
/// A ray that meets no face (`SkyVolume::hits`).
const OPEN: u32 = u32::MAX;

/// What a ray meets: the voxel, the axis of the face it enters by and the sign of its travel
/// along that axis (`d_k`), so the face looks along minus that sign.
fn face_code(voxel: usize, k: usize, d_k: f32) -> u32 {
    ((voxel as u32) << 3) | (2 * k as u32) | u32::from(d_k > 0.0)
}

/// Per sample of the grid `lo`, `cell`, `dims`: false inside a solid voxel (a wall or floor),
/// where a sample sees out of both sides, and in a gap thinner than `MIN_GAP_M` along an axis
/// (the hollow between the shells of a slab or a thick wall, under a sofa), which sees only
/// whatever its rays slip out to.
fn validity(lo: [f32; 3], cell: f32, dims: [u32; 3], voxels: &Voxels) -> Vec<bool> {
    let n = dims.iter().map(|&d| d as usize).product::<usize>();
    (0..n)
        .map(|i| {
            let at = [
                i % dims[0] as usize,
                i / dims[0] as usize % dims[1] as usize,
                i / (dims[0] * dims[1]) as usize,
            ];
            let p: [f32; 3] = std::array::from_fn(|k| lo[k] + cell * at[k] as f32);
            let gap = |k: usize| {
                let free = |sign: f32| {
                    let mut d = [0.0; 3];
                    d[k] = sign;
                    voxels.hit(p, d, MIN_GAP_M).map_or(MIN_GAP_M, |h| h.0)
                };
                free(1.0) + free(-1.0)
            };
            !voxels.solid_at(p) && (0..3).all(|k| gap(k) >= MIN_GAP_M)
        })
        .collect()
}
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
    /// Per sample: the sky's light passed round the room (module docs), as the illuminance of
    /// a surface that sees only room per unit of the sky's mean illuminance. -1 for a sample
    /// inside a wall or floor, which the renderer leaves out of its interpolation.
    pub room: Vec<f32>,
    voxels: Voxels,
    /// Per sample: which face neighbours it sees, bit `2 k` along -axis k, `2 k + 1` along +.
    links: Vec<u8>,
    /// Per sample, `RAYS` each: what the ray meets (`face_code`, or `OPEN`).
    hits: Vec<u32>,
    /// Per sample: not inside a wall or floor (`validity`; not stored in the bytes).
    valid: Vec<bool>,
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
        self.hits.iter().for_each(|&v| u(&mut b, v));
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
        let hits = floats(take(4 * n * RAYS)?)
            .iter()
            .map(|v| v.to_bits())
            .collect();
        let links = take(n)?.to_vec();
        let solid = take(nv)?.iter().map(|&v| v != 0).collect();
        if take(1).is_ok() {
            return Err("sky volume bytes too long".into());
        }
        let (origin, cell) = ([head[0], head[1], head[2]], head[3]);
        let voxels = Voxels {
            origin: [head[7], head[8], head[9]],
            dims: vdims,
            solid,
        };
        Ok(Self {
            origin,
            cell,
            dims,
            samples,
            room,
            valid: validity(origin, cell, dims, &voxels),
            voxels,
            links,
            hits,
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
    /// `room` as the renderer takes it: a sample inside a wall or a thin gap gets -1 minus the
    /// mean of the valid samples round it (3 x 3 x 3), for where nothing valid is near.
    pub fn room_texels(&self) -> Vec<f32> {
        let d = self.dims.map(|d| d as i64);
        (0..self.room.len())
            .map(|i| {
                if self.valid[i] {
                    return self.room[i];
                }
                let at = [
                    i as i64 % d[0],
                    i as i64 / d[0] % d[1],
                    i as i64 / (d[0] * d[1]),
                ];
                let (mut sum, mut n) = (0.0, 0.0);
                for k in 0..27i64 {
                    let q = [at[0] + k % 3 - 1, at[1] + k / 3 % 3 - 1, at[2] + k / 9 - 1];
                    if (0..3).all(|c| (0..d[c]).contains(&q[c])) {
                        let j = (q[0] + d[0] * (q[1] + d[1] * q[2])) as usize;
                        if self.valid[j] {
                            sum += self.room[j];
                            n += 1.0;
                        }
                    }
                }
                -1.0 - if n > 0.0 { sum / n } else { 0.0 }
            })
            .collect()
    }

    /// The occluders' voxels for the renderer (0 or 255, x fastest, from `origin`, `VOXEL_M`
    /// each) and their count per axis: what lets it interpolate only between samples that see
    /// the point.
    pub fn occupancy(&self) -> (Vec<u8>, [u32; 3]) {
        let d = self.voxels.dims;
        let bytes = self
            .voxels
            .solid
            .iter()
            .map(|&s| if s { 255 } else { 0 })
            .collect();
        (bytes, [d[0] as u32, d[1] as u32, d[2] as u32])
    }

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
                if let Some((t, k, _)) = self.voxels.hit(p, *d, REACH_M) {
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

    /// The sunlight let into the rooms passed round them, per sample: illuminance per unit of
    /// the sun's (normal to it) of a surface that sees only room. Parallel sun rays, one per
    /// voxel square of the box's sides that face the sun, mark the faces they reach (the sun's
    /// shadow, voxel by voxel); what those faces reflect is gathered round the rooms like the
    /// sky's light (`gather`): the sun patch on a floor lights the room round it. A coarse
    /// stand-in for the virtual lights of a reflective shadow map (Dachsbacher and Stamminger,
    /// "Reflective Shadow Maps", I3D 2005), made on the CPU when the sun moves.
    pub fn sun_bounce(&self, to_sun: [f32; 3]) -> Vec<f32> {
        let len = to_sun.iter().map(|c| c * c).sum::<f32>().sqrt();
        if len < 1e-6 || to_sun[1] <= 0.0 {
            return vec![0.0; self.samples.len()];
        }
        let l = to_sun.map(|c| c / len);
        let d = l.map(|c| -c);
        let vx = &self.voxels;
        // Per voxel, which of its six faces (`face_code`'s low bits) the sun reaches.
        let mut lit = vec![0u8; vx.solid.len()];
        for k in (0..3).filter(|&k| l[k].abs() > 1e-4) {
            let (u, w) = ((k + 1) % 3, (k + 2) % 3);
            let edge = if l[k] > 0.0 {
                vx.dims[k] as f32 * VOXEL_M - 1e-3
            } else {
                1e-3
            };
            for a in 0..vx.dims[u] {
                for b in 0..vx.dims[w] {
                    let mut p = vx.origin;
                    p[k] += edge;
                    p[u] += (a as f32 + 0.5) * VOXEL_M;
                    p[w] += (b as f32 + 0.5) * VOXEL_M;
                    if let Some((_, f, v)) = vx.hit(p, d, f32::MAX) {
                        let code = face_code(vx.index(v).expect("a hit is in the grid"), f, d[f]);
                        lit[(code >> 3) as usize] |= 1 << (code & 7);
                    }
                }
            }
        }
        self.gather(|code, _| {
            let k = ((code & 7) >> 1) as usize;
            if lit[(code >> 3) as usize] & (1 << (code & 7)) != 0 {
                l[k].abs()
            } else {
                0.0
            }
        })
    }

    /// The light passed round the rooms, per sample (module docs): `direct(face, seen)` is the
    /// light a face a ray meets receives from outside (`seen`: the sky its front sees); each
    /// bounce, the mean over a sample's rays that meet a face of that plus the bounced light the
    /// face gets from the room, reflected (`ROOM_REFLECTANCE`). Samples inside walls get none.
    fn gather(&self, direct: impl Fn(u32, f32) -> f32) -> Vec<f32> {
        let n = self.samples.len();
        let mut g = vec![0.0f32; n];
        for _ in 0..BOUNCES {
            g = (0..n)
                .map(|i| {
                    if !self.valid[i] {
                        return 0.0;
                    }
                    let (mut sum, mut met) = (0.0f32, 0.0f32);
                    for &code in &self.hits[i * RAYS..(i + 1) * RAYS] {
                        let Some((h, normal)) = self.face(code) else {
                            continue;
                        };
                        let seen = self.seen(h, normal);
                        sum += direct(code, seen) + g[h] * (1.0 - seen);
                        met += 1.0;
                    }
                    if met > 0.0 {
                        ROOM_REFLECTANCE * sum / met
                    } else {
                        0.0
                    }
                })
                .collect();
        }
        g
    }

    /// The face a ray met (`face_code`): the sample in front of it and its normal; `None` for
    /// an open ray or a face with no sample in the air before it.
    fn face(&self, code: u32) -> Option<(usize, [f32; 3])> {
        if code == OPEN {
            return None;
        }
        let d = self.voxels.dims;
        let v = (code >> 3) as i32;
        let v = [v % d[0], v / d[0] % d[1], v / (d[0] * d[1])];
        let k = ((code & 7) >> 1) as usize;
        let mut normal = [0.0f32; 3];
        normal[k] = if code & 1 == 1 { -1.0 } else { 1.0 };
        let reach = 0.5 * (VOXEL_M + self.cell);
        let p: [f32; 3] = std::array::from_fn(|c| {
            self.voxels.origin[c] + (v[c] as f32 + 0.5) * VOXEL_M + normal[c] * reach
        });
        let at = self.nearest(p)?;
        let i = (at[0] + self.dims[0] * (at[1] + self.dims[1] * at[2])) as usize;
        self.valid[i].then_some((i, normal))
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

    /// Whether `metres` along unit `d` from `p` hit nothing (leaving the grid counts as free).
    fn free(&self, p: [f32; 3], d: [f32; 3], metres: f32) -> bool {
        self.hit(p, d, metres).is_none()
    }

    /// Whether the voxel holding `p` is solid.
    fn solid_at(&self, p: [f32; 3]) -> bool {
        let v = std::array::from_fn(|k| ((p[k] - self.origin[k]) / VOXEL_M).floor() as i32);
        self.index(v).is_some_and(|i| self.solid[i])
    }

    /// The first solid voxel within `metres` along unit `d` from `p`: its distance (metres), the
    /// axis of the face the ray enters it by and the voxel (Amanatides and Woo, "A Fast Voxel
    /// Traversal Algorithm for Ray Tracing", Eurographics 1987).
    fn hit(&self, p: [f32; 3], d: [f32; 3], metres: f32) -> Option<(f32, usize, [i32; 3])> {
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
                Some(i) if self.solid[i] => return Some((t * VOXEL_M, k, v)),
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
    // The highest solid voxel of each column: a ray that comes down onto it meets ground, deck or
    // roof under the open sky, lit by the sun and the sky like the ground the ambient light's
    // lower half stands for (the sunlit deck seen through a glass wall), so it counts as open.
    let mut top = vec![-1i32; (vdims[0] * vdims[2]) as usize];
    for (i, _) in voxels.solid.iter().enumerate().filter(|s| *s.1) {
        let (x, y, z) = (
            i as i32 % vdims[0],
            i as i32 / vdims[0] % vdims[1],
            i as i32 / (vdims[0] * vdims[1]),
        );
        let t = &mut top[(x + vdims[0] * z) as usize];
        *t = (*t).max(y);
    }
    let meets = |p: [f32; 3], d: [f32; 3]| match voxels.hit(p, d, REACH_M) {
        None => OPEN,
        Some((_, 1, v)) if d[1] < 0.0 && top[(v[0] + vdims[0] * v[2]) as usize] == v[1] => OPEN,
        Some((_, k, v)) => face_code(voxels.index(v).expect("a hit is in the grid"), k, d[k]),
    };
    let dirs = directions(RAYS);
    // One z slab of samples (x fastest, then y), and what each one's rays meet.
    let slab = |z: u32| -> Vec<([f32; 4], [u32; RAYS])> {
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
                let mut met = [OPEN; RAYS];
                for (r, d) in dirs.iter().enumerate() {
                    let d = [c * d[0] - s * d[2], d[1], s * d[0] + c * d[2]];
                    met[r] = meets(p, d);
                    if met[r] == OPEN {
                        sum[0] += 1.0;
                        sum[1] += d[0];
                        sum[2] += d[1];
                        sum[3] += d[2];
                    }
                }
                out.push((sum.map(|v| v / RAYS as f32), met));
            }
        }
        out
    };
    // Natively the slabs are shared out over the cores; the browser has one thread (the app
    // runs this in a worker there).
    #[cfg(not(target_arch = "wasm32"))]
    let traced: Vec<([f32; 4], [u32; RAYS])> = {
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
    let traced: Vec<([f32; 4], [u32; RAYS])> = (0..dims[2]).flat_map(slab).collect();
    let samples: Vec<[f32; 4]> = traced.iter().map(|t| t.0).collect();
    let hits: Vec<u32> = traced.iter().flat_map(|t| t.1).collect();
    drop(traced);
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
    let point = |x: i64, y: i64, z: i64| -> [f32; 3] {
        [
            lo[0] + cell * x as f32,
            lo[1] + cell * y as f32,
            lo[2] + cell * z as f32,
        ]
    };
    let valid = validity(lo, cell, dims, &voxels);
    let mut smooth = samples.clone();
    for z in 0..dims[2] as i64 {
        for y in 0..dims[1] as i64 {
            for x in 0..dims[0] as i64 {
                let p = point(x, y, z);
                let (mut sum, mut n) = ([0.0f32; 4], 0.0);
                for (dx, dy, dz) in (0..27).map(|k| (k % 3 - 1, k / 3 % 3 - 1, k / 9 - 1)) {
                    let Some(i) = at(x + dx, y + dy, z + dz) else {
                        continue;
                    };
                    let d = [dx as f32, dy as f32, dz as f32];
                    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    if len > 0.0 && (!valid[i] || !voxels.free(p, d.map(|c| c / len), len * cell)) {
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
    // Which face neighbours each sample sees (both outside walls).
    let mut links = vec![0u8; smooth.len()];
    for z in 0..dims[2] as i64 {
        for y in 0..dims[1] as i64 {
            for x in 0..dims[0] as i64 {
                let p = point(x, y, z);
                let i = at(x, y, z).expect("in the grid");
                for b in 0..6 {
                    let mut o = [0i64; 3];
                    o[b / 2] = if b % 2 == 0 { -1 } else { 1 };
                    if valid[i]
                        && at(x + o[0], y + o[1], z + o[2]).is_some_and(|j| valid[j])
                        && voxels.free(p, o.map(|c| c as f32), cell)
                    {
                        links[i] |= 1 << b;
                    }
                }
            }
        }
    }
    let mut v = SkyVolume {
        origin: lo,
        cell,
        dims,
        samples: smooth,
        room: Vec::new(),
        voxels,
        links,
        hits,
        valid,
    };
    // The sky's light passed round the rooms: what the faces a sample's rays meet get of the sky.
    let room = v.gather(|_, seen| seen);
    v.room = (0..room.len())
        .map(|i| if v.valid[i] { room[i] } else { -1.0 })
        .collect();
    Some(v)
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
    fn sun_through_an_opening_lights_the_room_as_an_integrating_sphere_says() {
        let v = sky_volume(&room(2.0)).unwrap();
        let centre = v.index([3.0, 1.5, 3.0], [0.0; 3]).unwrap();
        // From the east, 30 degrees up, through the 2 x 3 m opening in the east wall.
        let (c, s) = (30f32.to_radians().cos(), 30f32.to_radians().sin());
        let e = v.sun_bounce([c, s, 0.0]);
        // The flux in, reflected once, over the 138 m2 of the room's surfaces (R / (1 - R) of it),
        // against the mean over the room's air (more over the sun patch, less away from it).
        let expected = ROOM_REFLECTANCE * c * 6.0 / (138.0 * (1.0 - ROOM_REFLECTANCE));
        let inside: Vec<f32> = (0..e.len())
            .filter(|&i| {
                let at = [
                    i as u32 % v.dims[0],
                    i as u32 / v.dims[0] % v.dims[1],
                    i as u32 / (v.dims[0] * v.dims[1]),
                ];
                let p: [f32; 3] = std::array::from_fn(|k| v.origin[k] + v.cell * at[k] as f32);
                p[0] > 0.3 && p[0] < 5.7 && p[1] > 0.3 && p[1] < 2.7 && p[2] > 0.3 && p[2] < 5.7
            })
            .map(|i| e[i])
            .collect();
        let mean = inside.iter().sum::<f32>() / inside.len() as f32;
        assert!(
            mean > 0.6 * expected && mean < 1.4 * expected,
            "{mean} {expected}"
        );
        // From the west the closed wall keeps it out, and the sun under the horizon gives none.
        assert!(v.sun_bounce([-c, s, 0.0])[centre] < 0.05 * expected);
        assert!(v.sun_bounce([c, -s, 0.0]).iter().all(|&e| e == 0.0));
    }

    /// Two 4 x 3 x 4 m rooms side by side along x with a 120 mm wall between them and a 1 m
    /// door in it; the east room has a 3 m opening in its east wall, the west room none.
    fn two_rooms() -> Vec<[[f32; 3]; 3]> {
        let mut t = Vec::new();
        t.extend(quad([0.0, 0.0, 0.0], [8.12, 0.0, 4.0]));
        t.extend(quad([0.0, 3.0, 0.0], [8.12, 3.0, 4.0]));
        t.extend(quad([0.0, 0.0, 0.0], [8.12, 3.0, 0.0]));
        t.extend(quad([0.0, 0.0, 4.0], [8.12, 3.0, 4.0]));
        t.extend(quad([0.0, 0.0, 0.0], [0.0, 3.0, 4.0]));
        for x in [4.0, 4.12] {
            t.extend(quad([x, 0.0, 0.0], [x, 3.0, 1.5]));
            t.extend(quad([x, 0.0, 2.5], [x, 3.0, 4.0]));
            t.extend(quad([x, 2.1, 1.5], [x, 3.0, 2.5]));
        }
        t.extend(quad([8.12, 0.0, 0.0], [8.12, 3.0, 0.5]));
        t.extend(quad([8.12, 0.0, 3.5], [8.12, 3.0, 4.0]));
        t
    }

    #[test]
    fn a_room_off_a_daylit_one_gets_less_light_the_further_from_the_door() {
        let v = sky_volume(&two_rooms()).unwrap();
        let room = |x: f32| v.room[v.index([x, 1.4, 2.0], [0.0; 3]).unwrap()];
        let (lit, door, mid, far) = (room(6.0), room(3.4), room(2.0), room(0.6));
        // Past the doorway the light spreads round the dark room, no longer falling much.
        assert!(
            lit > 3.0 * door && door > mid && door > far && far > 0.0,
            "{lit} {door} {mid} {far}"
        );
        // Either side of the wall beside the door: the dark room sees no sky, the lit one does.
        let by_wall = |x: f32| v.index([x, 1.4, 0.6], [0.0; 3]).unwrap();
        let (dark, lit_side) = (by_wall(3.7), by_wall(4.5));
        assert!(v.seen(dark, [-1.0, 0.0, 0.0]) < 0.02);
        assert!(v.seen(lit_side, [1.0, 0.0, 0.0]) > 0.05);
        // Samples inside the wall are marked, the air on either side is not.
        assert!(v.room.iter().any(|&r| r < 0.0));
        assert!(v.room[dark] >= 0.0 && v.room[lit_side] >= 0.0);
        let texels = v.room_texels();
        assert!(
            texels
                .iter()
                .zip(&v.room)
                .all(|(t, r)| (*r >= 0.0) == (*t >= 0.0))
        );
        assert!(texels.iter().all(|t| *t >= -1.0 - 0.99));
        // The renderer's voxels: the wall between the rooms is solid, the air either side not.
        let (solid, d) = v.occupancy();
        let voxel = |p: [f32; 3]| {
            let c: [usize; 3] = std::array::from_fn(|k| ((p[k] - v.origin[k]) / VOXEL_M) as usize);
            solid[c[0] + d[0] as usize * (c[1] + d[1] as usize * c[2])]
        };
        assert_eq!(voxel([4.05, 1.4, 0.6]), 255);
        assert_eq!(voxel([3.7, 1.4, 0.6]) + voxel([4.5, 1.4, 0.6]), 0);
        // Low sun from the east into the lit room passes round it and, less, through the door.
        let sun = v.sun_bounce([0.9, 0.44, 0.0]);
        let at = |x: f32| sun[v.index([x, 1.4, 2.0], [0.0; 3]).unwrap()];
        assert!(at(6.0) > at(3.0) && at(3.0) > at(0.6) && at(0.6) >= 0.0);
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
