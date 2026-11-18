//! Ground with relief: a height grid with painted material weights, sculpting brushes, queries
//! and chunked meshes for the renderer.
//!
//! In the project JSON the two big arrays are stored compactly: little-endian bytes, byte-shuffled
//! (all first bytes, then all second bytes ...: neighbours in a smooth surface share their high
//! bytes), deflate-compressed with `miniz_oxide` and base64-encoded. Lossless.

use crate::ImportedModel;
use crate::planting::{Rng, mix, point_in_polygon};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Most vertices per side (1024 cells).
pub const MAX_VERTICES_PER_SIDE: u32 = 1025;
/// Most material layers.
pub const MAX_LAYERS: usize = 8;
/// Cells per side of one mesh chunk.
pub const CHUNK_CELLS: u32 = 64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerrainLayer {
    /// A material id.
    pub material: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Terrain {
    /// World `(x, z)` of the corner of cell (0, 0).
    pub origin: [f32; 2],
    pub cell_m: f32,
    /// Cells; there are `width + 1` by `depth + 1` vertices.
    pub width: u32,
    pub depth: u32,
    /// Metres, row-major: `heights[iz * (width + 1) + ix]`.
    pub heights: Vec<f32>,
    /// At most [`MAX_LAYERS`]; layer 0 is the base.
    pub layers: Vec<TerrainLayer>,
    /// Per vertex, per layer: `weights[vertex * layers.len() + layer]`; each vertex sums to 255.
    pub weights: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerrainError(pub String);

impl fmt::Display for TerrainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TerrainError {}

fn err<T>(why: impl Into<String>) -> Result<T, TerrainError> {
    Err(TerrainError(why.into()))
}

// ---- compact encoding ----

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for k in 0..4 {
            if k <= c.len() {
                out.push(B64[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_decode(text: &str) -> Result<Vec<u8>, TerrainError> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for b in text.bytes().take_while(|b| *b != b'=') {
        let v = B64
            .iter()
            .position(|c| *c == b)
            .ok_or_else(|| TerrainError("bad base64".into()))? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// Groups byte `k` of every `stride`-byte element together.
fn shuffle(bytes: &[u8], stride: usize) -> Vec<u8> {
    let n = bytes.len() / stride;
    let mut out = vec![0; bytes.len()];
    for i in 0..n {
        for k in 0..stride {
            out[k * n + i] = bytes[i * stride + k];
        }
    }
    out
}

fn unshuffle(bytes: &[u8], stride: usize) -> Vec<u8> {
    let n = bytes.len() / stride;
    let mut out = vec![0; bytes.len()];
    for i in 0..n {
        for k in 0..stride {
            out[i * stride + k] = bytes[k * n + i];
        }
    }
    out
}

fn pack(bytes: &[u8], stride: usize) -> String {
    base64_encode(&miniz_oxide::deflate::compress_to_vec(
        &shuffle(bytes, stride),
        6,
    ))
}

fn unpack(text: &str, stride: usize, expected: usize) -> Result<Vec<u8>, TerrainError> {
    let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(&base64_decode(text)?, expected)
        .map_err(|e| TerrainError(format!("bad terrain data: {e:?}")))?;
    if raw.len() != expected {
        return err("terrain data has the wrong length");
    }
    Ok(unshuffle(&raw, stride))
}

#[derive(Serialize, Deserialize)]
struct Repr {
    origin: [f32; 2],
    cell_m: f32,
    width: u32,
    depth: u32,
    layers: Vec<TerrainLayer>,
    /// f32 little-endian, shuffled by 4, deflated, base64.
    heights: String,
    /// u8, shuffled by the layer count, deflated, base64.
    weights: String,
}

impl Serialize for Terrain {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let bytes: Vec<u8> = self.heights.iter().flat_map(|h| h.to_le_bytes()).collect();
        Repr {
            origin: self.origin,
            cell_m: self.cell_m,
            width: self.width,
            depth: self.depth,
            layers: self.layers.clone(),
            heights: pack(&bytes, 4),
            weights: pack(&self.weights, self.layers.len().max(1)),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for Terrain {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = Repr::deserialize(d)?;
        let bad = |m: String| <D::Error as serde::de::Error>::custom(m);
        if r.width == 0
            || r.depth == 0
            || r.width >= MAX_VERTICES_PER_SIDE
            || r.depth >= MAX_VERTICES_PER_SIDE
        {
            return Err(bad("terrain size out of range".into()));
        }
        if r.layers.is_empty() || r.layers.len() > MAX_LAYERS {
            return Err(bad("terrain needs 1 to 8 layers".into()));
        }
        let vertices = (r.width as usize + 1) * (r.depth as usize + 1);
        let h = unpack(&r.heights, 4, vertices * 4).map_err(|e| bad(e.0))?;
        let heights = h
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        let weights =
            unpack(&r.weights, r.layers.len(), vertices * r.layers.len()).map_err(|e| bad(e.0))?;
        let t = Terrain {
            origin: r.origin,
            cell_m: r.cell_m,
            width: r.width,
            depth: r.depth,
            heights,
            layers: r.layers,
            weights,
        };
        t.validate().map_err(|e| bad(e.0))?;
        Ok(t)
    }
}

// ---- construction and queries ----

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Integer weights summing to 255 that are as close as possible to the proportions of `f`
/// (largest remainder; ties go to the lowest index). All zero gives the first layer everything.
fn normalize255(f: &[f32]) -> Vec<u8> {
    let total: f32 = f.iter().map(|v| v.max(0.0)).sum();
    if total.is_nan() || total <= 0.0 {
        let mut out = vec![0; f.len()];
        out[0] = 255;
        return out;
    }
    let scaled: Vec<f32> = f.iter().map(|v| v.max(0.0) / total * 255.0).collect();
    let mut out: Vec<u8> = scaled.iter().map(|v| *v as u8).collect();
    let mut left = 255 - out.iter().map(|v| u32::from(*v)).sum::<u32>();
    let mut order: Vec<usize> = (0..f.len()).collect();
    order.sort_by(|&a, &b| {
        (scaled[b] - scaled[b].floor())
            .total_cmp(&(scaled[a] - scaled[a].floor()))
            .then(a.cmp(&b))
    });
    for i in order {
        if left == 0 {
            break;
        }
        out[i] += 1;
        left -= 1;
    }
    out
}

impl Terrain {
    /// A flat terrain at height 0 covering at least `width_m` by `depth_m`, all of it the base material.
    pub fn flat(
        origin: [f32; 2],
        width_m: f32,
        depth_m: f32,
        cell_m: f32,
        base_material: &str,
    ) -> Result<Self, TerrainError> {
        if !(cell_m.is_finite()
            && cell_m > 0.0
            && width_m.is_finite()
            && width_m > 0.0
            && depth_m.is_finite()
            && depth_m > 0.0
            && origin.iter().all(|c| c.is_finite()))
        {
            return err("origin, size and cell size must be finite and above zero");
        }
        let (w, d) = ((width_m / cell_m).ceil(), (depth_m / cell_m).ceil());
        if w >= MAX_VERTICES_PER_SIDE as f32 || d >= MAX_VERTICES_PER_SIDE as f32 {
            return err("a terrain is at most 1024 cells on a side");
        }
        let (width, depth) = (w as u32, d as u32);
        let n = (width as usize + 1) * (depth as usize + 1);
        Ok(Self {
            origin,
            cell_m,
            width,
            depth,
            heights: vec![0.0; n],
            layers: vec![TerrainLayer {
                material: base_material.into(),
            }],
            weights: vec![255; n],
        })
    }

    /// Checks the invariants the rest of the code relies on.
    pub fn validate(&self) -> Result<(), TerrainError> {
        let side = |c: u32| (1..MAX_VERTICES_PER_SIDE).contains(&c);
        if !(side(self.width)
            && side(self.depth)
            && self.cell_m.is_finite()
            && self.cell_m > 0.0
            && self.origin.iter().all(|c| c.is_finite()))
        {
            return err("terrain size or origin out of range");
        }
        let n = self.vertex_count();
        let l = self.layers.len();
        if l == 0 || l > MAX_LAYERS || self.heights.len() != n || self.weights.len() != n * l {
            return err("terrain arrays do not match its size and layers");
        }
        if self.heights.iter().any(|h| !h.is_finite()) {
            return err("terrain heights must be finite");
        }
        if self
            .weights
            .chunks_exact(l)
            .any(|w| w.iter().map(|v| u32::from(*v)).sum::<u32>() != 255)
        {
            return err("terrain weights must sum to 255 at every vertex");
        }
        Ok(())
    }

    /// All vertices, as a dirty rectangle.
    pub fn whole(&self) -> DirtyRect {
        DirtyRect {
            x0: 0,
            z0: 0,
            x1: self.width,
            z1: self.depth,
        }
    }

    /// The vertices whose height or paint differ from `old`: their bounding rectangle, `None` if
    /// there is no difference; everything when the grid or layers themselves changed.
    pub fn changed_since(&self, old: &Terrain) -> Option<DirtyRect> {
        if (self.origin, self.cell_m, self.width, self.depth)
            != (old.origin, old.cell_m, old.width, old.depth)
            || self.layers.len() != old.layers.len()
        {
            return Some(self.whole());
        }
        let n = self.layers.len();
        let mut dirty = DirtyRect::EMPTY;
        for iz in 0..=self.depth {
            for ix in 0..=self.width {
                let i = self.at(ix, iz);
                if self.heights[i] != old.heights[i]
                    || self.weights[i * n..(i + 1) * n] != old.weights[i * n..(i + 1) * n]
                {
                    dirty = dirty.union(DirtyRect {
                        x0: ix,
                        z0: iz,
                        x1: ix,
                        z1: iz,
                    });
                }
            }
        }
        let layers_changed = self.layers != old.layers;
        if dirty.is_empty() && !layers_changed {
            None
        } else if layers_changed {
            Some(self.whole())
        } else {
            Some(dirty)
        }
    }

    pub fn vertex_count(&self) -> usize {
        (self.width as usize + 1) * (self.depth as usize + 1)
    }

    fn at(&self, ix: u32, iz: u32) -> usize {
        iz as usize * (self.width as usize + 1) + ix as usize
    }

    /// World position of vertex `(ix, iz)`, without its height.
    fn xz(&self, ix: u32, iz: u32) -> [f32; 2] {
        [
            self.origin[0] + ix as f32 * self.cell_m,
            self.origin[1] + iz as f32 * self.cell_m,
        ]
    }

    /// World extent `[min, max]` in x and z.
    pub fn extent(&self) -> [[f32; 2]; 2] {
        [
            self.origin,
            [
                self.origin[0] + self.width as f32 * self.cell_m,
                self.origin[1] + self.depth as f32 * self.cell_m,
            ],
        ]
    }

    fn height_clamped(&self, ix: i64, iz: i64) -> f32 {
        let (x, z) = (
            ix.clamp(0, i64::from(self.width)) as u32,
            iz.clamp(0, i64::from(self.depth)) as u32,
        );
        self.heights[self.at(x, z)]
    }

    /// Bilinear height at world `(x, z)`; `None` outside the terrain.
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        let (u, v) = (
            (x - self.origin[0]) / self.cell_m,
            (z - self.origin[1]) / self.cell_m,
        );
        if !(u >= 0.0 && v >= 0.0 && u <= self.width as f32 && v <= self.depth as f32) {
            return None;
        }
        Some(self.height_uv(u, v))
    }

    fn height_uv(&self, u: f32, v: f32) -> f32 {
        let (ix, iz) = (
            (u.floor() as i64).min(i64::from(self.width) - 1),
            (v.floor() as i64).min(i64::from(self.depth) - 1),
        );
        let (fx, fz) = (u - ix as f32, v - iz as f32);
        let h = |dx, dz| self.height_clamped(ix + dx, iz + dz);
        let (a, b) = (
            h(0, 0) + (h(1, 0) - h(0, 0)) * fx,
            h(0, 1) + (h(1, 1) - h(0, 1)) * fx,
        );
        a + (b - a) * fz
    }

    /// Unit surface normal (pointing up) at world `(x, z)`, from central differences; points outside
    /// the terrain use the nearest edge.
    pub fn normal_at(&self, x: f32, z: f32) -> [f32; 3] {
        let (u, v) = (
            (x - self.origin[0]) / self.cell_m,
            (z - self.origin[1]) / self.cell_m,
        );
        let (u, v) = (
            u.clamp(0.0, self.width as f32),
            v.clamp(0.0, self.depth as f32),
        );
        let e = 0.5;
        let (dx, dz) = (
            self.height_uv((u + e).min(self.width as f32), v) - self.height_uv((u - e).max(0.0), v),
            self.height_uv(u, (v + e).min(self.depth as f32)) - self.height_uv(u, (v - e).max(0.0)),
        );
        let (sx, sz) = (
            ((u + e).min(self.width as f32) - (u - e).max(0.0)) * self.cell_m,
            ((v + e).min(self.depth as f32) - (v - e).max(0.0)) * self.cell_m,
        );
        normalise([-dx / sx, 1.0, -dz / sz])
    }

    /// Material weights at the nearest vertex of world `(x, z)`, one per layer; `None` outside.
    pub fn weights_at(&self, x: f32, z: f32) -> Option<&[u8]> {
        let (u, v) = (
            (x - self.origin[0]) / self.cell_m,
            (z - self.origin[1]) / self.cell_m,
        );
        if !(u >= 0.0 && v >= 0.0 && u <= self.width as f32 && v <= self.depth as f32) {
            return None;
        }
        let i = self.at(u.round() as u32, v.round() as u32) * self.layers.len();
        Some(&self.weights[i..i + self.layers.len()])
    }

    /// The first hit of the ray `origin + dir * t` (`t >= 0`) with the surface, as `(t, point,
    /// normal)`; the normal faces up. Walks the cells the ray crosses in order (a 2D grid
    /// traversal) and tests the two triangles of each (the surface the meshes draw, which differs
    /// slightly from the bilinear `height_at` inside a cell).
    pub fn raycast(&self, origin: [f32; 3], dir: [f32; 3]) -> Option<(f32, [f32; 3], [f32; 3])> {
        self.raycast_where(origin, dir, &|_| true)
    }

    /// [`Terrain::raycast`] where a hit only counts if `accept(point)`: the first accepted one
    /// (a terrace's footprint is cut out of the terrain: the ray goes on through it).
    pub fn raycast_where(
        &self,
        origin: [f32; 3],
        dir: [f32; 3],
        accept: &dyn Fn([f32; 3]) -> bool,
    ) -> Option<(f32, [f32; 3], [f32; 3])> {
        let ext = self.extent();
        // Clip the ray to the terrain's footprint.
        let (mut t0, mut t1) = (0.0f32, f32::INFINITY);
        for (k, d) in [(0, dir[0]), (1, dir[2])] {
            let o = if k == 0 { origin[0] } else { origin[2] };
            if d == 0.0 {
                if o < ext[0][k] || o > ext[1][k] {
                    return None;
                }
            } else {
                let (a, b) = ((ext[0][k] - o) / d, (ext[1][k] - o) / d);
                t0 = t0.max(a.min(b));
                t1 = t1.min(a.max(b));
            }
        }
        if t0 > t1 {
            return None;
        }
        let at = |t: f32| [origin[0] + dir[0] * t, origin[2] + dir[2] * t];
        let p = at(t0);
        let (mut ix, mut iz) = (
            (((p[0] - self.origin[0]) / self.cell_m).floor() as i64)
                .clamp(0, i64::from(self.width) - 1),
            (((p[1] - self.origin[1]) / self.cell_m).floor() as i64)
                .clamp(0, i64::from(self.depth) - 1),
        );
        let step = |d: f32| if d > 0.0 { 1 } else { -1 };
        let (sx, sz) = (step(dir[0]), step(dir[2]));
        // Distance (in t) to the next cell border in x and z, and between borders.
        let next = |i: i64, s: i64, d: f32, o: f32, org: f32| {
            if d == 0.0 {
                f32::INFINITY
            } else {
                let border = org + (if s > 0 { i + 1 } else { i }) as f32 * self.cell_m;
                (border - o) / d
            }
        };
        let mut t_max = [
            next(ix, sx, dir[0], origin[0], self.origin[0]),
            next(iz, sz, dir[2], origin[2], self.origin[1]),
        ];
        let t_delta = [
            if dir[0] == 0.0 {
                f32::INFINITY
            } else {
                self.cell_m / dir[0].abs()
            },
            if dir[2] == 0.0 {
                f32::INFINITY
            } else {
                self.cell_m / dir[2].abs()
            },
        ];
        loop {
            if let Some(hit) = self.hit_cell(ix as u32, iz as u32, origin, dir)
                && accept(hit.1)
            {
                return Some(hit);
            }
            let k = if t_max[0] < t_max[1] { 0 } else { 1 };
            if t_max[k] > t1 {
                return None;
            }
            if k == 0 {
                ix += sx;
                t_max[0] += t_delta[0];
            } else {
                iz += sz;
                t_max[1] += t_delta[1];
            }
            if ix < 0 || iz < 0 || ix >= i64::from(self.width) || iz >= i64::from(self.depth) {
                return None;
            }
        }
    }

    /// Height at world `(x, z)` on the triangles the terrain is drawn with (each cell split along
    /// its (0,0)-(1,1) diagonal), not the bilinear [`Terrain::height_at`]; `None` outside.
    pub fn surface_height(&self, x: f32, z: f32) -> Option<f32> {
        let (u, v) = (
            (x - self.origin[0]) / self.cell_m,
            (z - self.origin[1]) / self.cell_m,
        );
        if !(u >= 0.0 && v >= 0.0 && u <= self.width as f32 && v <= self.depth as f32) {
            return None;
        }
        let (ix, iz) = (
            (u.floor() as i64).min(i64::from(self.width) - 1),
            (v.floor() as i64).min(i64::from(self.depth) - 1),
        );
        let (fx, fz) = (u - ix as f32, v - iz as f32);
        let h = |dx, dz| self.height_clamped(ix + dx, iz + dz);
        // Above the diagonal (fz > fx): corners (0,0), (0,1), (1,1); below: (0,0), (1,1), (1,0).
        Some(if fz >= fx {
            h(0, 0) + (h(0, 1) - h(0, 0)) * fz + (h(1, 1) - h(0, 1)) * fx
        } else {
            h(0, 0) + (h(1, 0) - h(0, 0)) * fx + (h(1, 1) - h(1, 0)) * fz
        })
    }

    /// The two triangles of a cell, split along the diagonal from corner (0,0) to (1,1).
    fn cell_triangles(&self, ix: u32, iz: u32) -> [[[f32; 3]; 3]; 2] {
        let v = |dx, dz| {
            let p = self.xz(ix + dx, iz + dz);
            [p[0], self.heights[self.at(ix + dx, iz + dz)], p[1]]
        };
        [[v(0, 0), v(0, 1), v(1, 1)], [v(0, 0), v(1, 1), v(1, 0)]]
    }

    fn hit_cell(
        &self,
        ix: u32,
        iz: u32,
        origin: [f32; 3],
        dir: [f32; 3],
    ) -> Option<(f32, [f32; 3], [f32; 3])> {
        let mut best: Option<(f32, [f32; 3])> = None;
        for tri in self.cell_triangles(ix, iz) {
            let (e1, e2) = (sub(tri[1], tri[0]), sub(tri[2], tri[0]));
            let h = cross(dir, e2);
            let det = dot(e1, h);
            if det.abs() < 1e-12 {
                continue;
            }
            let s = sub(origin, tri[0]);
            let u = dot(s, h) / det;
            let q = cross(s, e1);
            let v = dot(dir, q) / det;
            let t = dot(e2, q) / det;
            // A hair of slack so a ray through a shared edge does not slip between triangles.
            if u >= -1e-6
                && v >= -1e-6
                && u + v <= 1.0 + 1e-6
                && t >= 0.0
                && best.is_none_or(|(b, _)| t < b)
            {
                let n = normalise(cross(e1, e2));
                best = Some((t, if n[1] < 0.0 { n.map(|c| -c) } else { n }));
            }
        }
        best.map(|(t, n)| (t, std::array::from_fn(|k| origin[k] + dir[k] * t), n))
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalise(v: [f32; 3]) -> [f32; 3] {
    let l = dot(v, v).sqrt();
    if l > 0.0 {
        v.map(|c| c / l)
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// The height the app should use as the ground callback: the highest of the terrain and of the
/// models' surfaces at `(x, z)` (a house's roof beats the lawn beside it), `None` where there is
/// nothing. Models are tested with a ray straight down, so a roof is "ground" for anything placed
/// over it; the app decides which models take part.
pub fn ground_height_over(
    terrain: Option<&Terrain>,
    models: &[&ImportedModel],
    x: f32,
    z: f32,
) -> Option<f32> {
    terrain
        .and_then(|t| t.height_at(x, z))
        .into_iter()
        .chain(
            models
                .iter()
                .filter_map(|m| crate::raycast::ground_height(m, x, z)),
        )
        .reduce(f32::max)
}

// ---- brushes ----

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    Raise,
    Lower,
    /// Towards `target_height`, or the height under the first point.
    Flatten,
    Smooth,
    /// Deterministic value noise, seeded from the stroke.
    Noise,
    /// Moves weight towards `layer`.
    Paint,
    /// A linear slope between the first and last point across the brush width.
    Ramp,
}

/// One brush stroke: a path of dabs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushStroke {
    pub tool: Tool,
    pub points: Vec<[f32; 2]>,
    pub radius_m: f32,
    /// Raise, lower and noise: metres per dab at full weight. The others: the fraction (0..1) moved
    /// towards the goal per dab.
    pub strength: f32,
    /// 0..1: the brush is full strength inside `radius * (1 - falloff)` and fades with a smoothstep to the edge.
    pub falloff: f32,
    #[serde(default)]
    pub target_height: Option<f32>,
    /// The layer to paint.
    #[serde(default)]
    pub layer: Option<usize>,
}

impl BrushStroke {
    pub fn validate(&self, terrain: &Terrain) -> Result<(), TerrainError> {
        let finite = self.points.iter().flatten().all(|c| c.is_finite())
            && self.radius_m.is_finite()
            && self.strength.is_finite()
            && self.target_height.is_none_or(f32::is_finite);
        if self.points.is_empty()
            || !finite
            || self.radius_m <= 0.0
            || !(0.0..=1.0).contains(&self.falloff)
        {
            return err("a stroke needs points, a radius above zero and a falloff in 0..1");
        }
        if self.tool == Tool::Paint && !self.layer.is_some_and(|l| l < terrain.layers.len()) {
            return err("paint needs an existing layer");
        }
        Ok(())
    }
}

/// Vertex rectangle `[x0, z0]..=[x1, z1]` a change touched; empty when `x0 > x1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirtyRect {
    pub x0: u32,
    pub z0: u32,
    pub x1: u32,
    pub z1: u32,
}

impl DirtyRect {
    pub const EMPTY: Self = Self {
        x0: u32::MAX,
        z0: u32::MAX,
        x1: 0,
        z1: 0,
    };

    pub fn is_empty(&self) -> bool {
        self.x0 > self.x1 || self.z0 > self.z1
    }

    /// The smallest rectangle holding both (an empty one adds nothing).
    pub fn union(self, other: Self) -> Self {
        if other.is_empty() {
            return self;
        }
        if self.is_empty() {
            return other;
        }
        Self {
            x0: self.x0.min(other.x0),
            z0: self.z0.min(other.z0),
            x1: self.x1.max(other.x1),
            z1: self.z1.max(other.z1),
        }
    }

    fn add(&mut self, other: Self) {
        *self = self.union(other);
    }
}

/// Brush weight at distance `d`: 1 inside the inner radius, a smoothstep down to 0 at the edge.
fn brush_weight(d: f32, radius: f32, falloff: f32) -> f32 {
    let inner = radius * (1.0 - falloff);
    if d <= inner {
        1.0
    } else if d >= radius {
        0.0
    } else {
        1.0 - smoothstep((d - inner) / (radius - inner))
    }
}

impl Terrain {
    /// Vertex index range covering a disc (or box) of `r` around `c`, clamped; `None` if outside.
    fn vertex_box(&self, lo: [f32; 2], hi: [f32; 2]) -> Option<DirtyRect> {
        let range = |a: f32, b: f32, org: f32, n: u32| {
            let (i0, i1) = (
                ((a - org) / self.cell_m).ceil().max(0.0),
                ((b - org) / self.cell_m).floor().min(n as f32),
            );
            (i0 <= i1).then_some((i0 as u32, i1 as u32))
        };
        let (x0, x1) = range(lo[0], hi[0], self.origin[0], self.width)?;
        let (z0, z1) = range(lo[1], hi[1], self.origin[1], self.depth)?;
        Some(DirtyRect { x0, z0, x1, z1 })
    }

    fn average_around(&self, ix: u32, iz: u32) -> f32 {
        let mut sum = 0.0;
        for (dx, dz) in [
            (-1, -1),
            (0, -1),
            (1, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ] {
            sum += self.height_clamped(i64::from(ix) + dx, i64::from(iz) + dz);
        }
        sum / 8.0
    }

    fn paint_vertex(&mut self, v: usize, layer: usize, amount: f32) {
        let n = self.layers.len();
        let w: Vec<f32> = self.weights[v * n..(v + 1) * n]
            .iter()
            .map(|b| f32::from(*b))
            .collect();
        if w[layer] >= 255.0 {
            return;
        }
        let new = w[layer] + (255.0 - w[layer]) * amount;
        let rest = (255.0 - new) / (255.0 - w[layer]);
        let f: Vec<f32> = w
            .iter()
            .enumerate()
            .map(|(i, x)| if i == layer { new } else { x * rest })
            .collect();
        self.weights[v * n..(v + 1) * n].copy_from_slice(&normalize255(&f));
    }
}

/// Applies a whole stroke in place and returns the vertices it touched. This is what the
/// `terrain.stroke` command runs; the app can run it on a scratch copy to preview. A stroke that
/// does not validate changes nothing. Dabs are spaced a quarter radius apart (at most half a
/// cell), so a dab's `strength` accumulates; use small values.
pub fn apply_stroke(t: &mut Terrain, s: &BrushStroke) -> DirtyRect {
    let mut dirty = DirtyRect::EMPTY;
    if s.validate(t).is_err() {
        return dirty;
    }
    if s.tool == Tool::Ramp {
        return ramp(t, s);
    }
    let target = s.target_height.unwrap_or_else(|| {
        let p = s.points[0];
        t.height_at(p[0], p[1]).unwrap_or(0.0)
    });
    let seed = s.points.iter().fold(s.tool as u64 + 1, |h, p| {
        mix(mix(h, i64::from(p[0].to_bits())), i64::from(p[1].to_bits()))
    });
    let noise_scale = (s.radius_m * 0.5).max(t.cell_m);
    for c in dab_centres(&s.points, (s.radius_m * 0.25).max(t.cell_m * 0.5)) {
        let Some(b) = t.vertex_box(
            [c[0] - s.radius_m, c[1] - s.radius_m],
            [c[0] + s.radius_m, c[1] + s.radius_m],
        ) else {
            continue;
        };
        dirty.add(b);
        for iz in b.z0..=b.z1 {
            for ix in b.x0..=b.x1 {
                let p = t.xz(ix, iz);
                let w = brush_weight((p[0] - c[0]).hypot(p[1] - c[1]), s.radius_m, s.falloff);
                if w <= 0.0 {
                    continue;
                }
                let i = t.at(ix, iz);
                let amount = (s.strength * w).clamp(-1.0, 1.0);
                match s.tool {
                    Tool::Raise => t.heights[i] += s.strength * w,
                    Tool::Lower => t.heights[i] -= s.strength * w,
                    Tool::Flatten => t.heights[i] += (target - t.heights[i]) * amount.max(0.0),
                    Tool::Smooth => {
                        let avg = t.average_around(ix, iz);
                        t.heights[i] += (avg - t.heights[i]) * amount.max(0.0);
                    }
                    Tool::Noise => {
                        t.heights[i] += s.strength
                            * w
                            * value_noise(seed, p[0] / noise_scale, p[1] / noise_scale)
                    }
                    Tool::Paint => t.paint_vertex(i, s.layer.unwrap_or(0), amount.max(0.0)),
                    Tool::Ramp => {}
                }
            }
        }
    }
    dirty
}

/// Dab positions along a polyline, `spacing` apart, ending on the last point.
fn dab_centres(points: &[[f32; 2]], spacing: f32) -> Vec<[f32; 2]> {
    let mut out = vec![points[0]];
    let mut carry = 0.0f32; // distance walked since the last dab
    for w in points.windows(2) {
        let len = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
        let mut at = spacing - carry;
        while at <= len {
            let t = at / len;
            out.push([
                w[0][0] + (w[1][0] - w[0][0]) * t,
                w[0][1] + (w[1][1] - w[0][1]) * t,
            ]);
            at += spacing;
        }
        carry = len - (at - spacing);
    }
    if points.len() > 1 && carry > 1e-4 {
        out.push(points[points.len() - 1]);
    }
    out
}

/// Value noise in `-1..1`: hashed lattice values, smoothly interpolated.
pub(crate) fn value_noise(seed: u64, x: f32, z: f32) -> f32 {
    let (ix, iz) = (x.floor(), z.floor());
    let (fx, fz) = (smoothstep(x - ix), smoothstep(z - iz));
    let v = |dx: i64, dz: i64| {
        Rng::new(mix(mix(seed, ix as i64 + dx), iz as i64 + dz)).f32() * 2.0 - 1.0
    };
    let (a, b) = (
        v(0, 0) + (v(1, 0) - v(0, 0)) * fx,
        v(0, 1) + (v(1, 1) - v(0, 1)) * fx,
    );
    a + (b - a) * fz
}

fn ramp(t: &mut Terrain, s: &BrushStroke) -> DirtyRect {
    let (a, b) = (s.points[0], s.points[s.points.len() - 1]);
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dz * dz;
    let mut dirty = DirtyRect::EMPTY;
    if len2 == 0.0 {
        return dirty;
    }
    let clamped = |p: [f32; 2]| {
        let u = ((p[0] - t.origin[0]) / t.cell_m).clamp(0.0, t.width as f32);
        let v = ((p[1] - t.origin[1]) / t.cell_m).clamp(0.0, t.depth as f32);
        t.height_uv(u, v)
    };
    let (ha, hb) = (clamped(a), clamped(b));
    let r = s.radius_m;
    let Some(bx) = t.vertex_box(
        [a[0].min(b[0]) - r, a[1].min(b[1]) - r],
        [a[0].max(b[0]) + r, a[1].max(b[1]) + r],
    ) else {
        return dirty;
    };
    dirty.add(bx);
    let amount = s.strength.clamp(0.0, 1.0);
    for iz in bx.z0..=bx.z1 {
        for ix in bx.x0..=bx.x1 {
            let p = t.xz(ix, iz);
            let u = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dz) / len2).clamp(0.0, 1.0);
            let d = (p[0] - a[0] - dx * u).hypot(p[1] - a[1] - dz * u);
            let w = brush_weight(d, r, s.falloff);
            let i = t.at(ix, iz);
            t.heights[i] += (ha + (hb - ha) * u - t.heights[i]) * w * amount;
        }
    }
    dirty
}

// ---- shape-driven edits ----

/// Distance from `p` to the outline, and whether `p` is inside.
fn outline_distance(p: [f32; 2], outline: &[[f32; 2]]) -> (f32, bool) {
    let mut best = f32::INFINITY;
    for i in 0..outline.len() {
        let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
        let (ab, ap) = ([b[0] - a[0], b[1] - a[1]], [p[0] - a[0], p[1] - a[1]]);
        let len2 = ab[0] * ab[0] + ab[1] * ab[1];
        let u = if len2 > 0.0 {
            ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        best = best.min((ap[0] - ab[0] * u).hypot(ap[1] - ab[1] * u));
    }
    (best, point_in_polygon(p, outline))
}

impl Terrain {
    /// Sets the ground inside `outline` to `height`, blending to the old ground over `margin_m` outside it.
    pub fn level_under(&mut self, outline: &[[f32; 2]], height: f32, margin_m: f32) -> DirtyRect {
        self.edit_outline(outline, margin_m, |h, t| {
            if t >= 1.0 {
                height
            } else {
                h + (height - h) * t
            }
        })
    }

    /// Lowers the ground by `depth_m` inside `outline`, with banks sloping up over `bank_m` outside it.
    pub fn carve(&mut self, outline: &[[f32; 2]], depth_m: f32, bank_m: f32) -> DirtyRect {
        self.edit_outline(outline, bank_m, |h, t| h - depth_m * t)
    }

    /// Paints `layer` into the ground inside `outline` and over `margin` outside it, fading out
    /// across the margin: the bed of a carved pond.
    pub fn paint_outline(&mut self, outline: &[[f32; 2]], margin: f32, layer: usize) -> DirtyRect {
        if layer >= self.layers.len() {
            return DirtyRect::EMPTY;
        }
        let b = bounds(outline, margin.max(0.0));
        let Some(r) = self.vertex_box(b[0], b[1]) else {
            return DirtyRect::EMPTY;
        };
        for iz in r.z0..=r.z1 {
            for ix in r.x0..=r.x1 {
                let p = self.xz(ix, iz);
                let (d, inside) = outline_distance(p, outline);
                let t = if inside {
                    1.0
                } else if margin > 0.0 && d < margin {
                    1.0 - smoothstep(d / margin)
                } else {
                    continue;
                };
                let v = self.at(ix, iz);
                self.paint_vertex(v, layer, t);
            }
        }
        r
    }

    /// `f(old height, t)` with `t` 1 inside the outline and falling smoothly to 0 `margin` outside.
    fn edit_outline(
        &mut self,
        outline: &[[f32; 2]],
        margin: f32,
        f: impl Fn(f32, f32) -> f32,
    ) -> DirtyRect {
        let b = bounds(outline, margin.max(0.0));
        let Some(r) = self.vertex_box(b[0], b[1]) else {
            return DirtyRect::EMPTY;
        };
        for iz in r.z0..=r.z1 {
            for ix in r.x0..=r.x1 {
                let p = self.xz(ix, iz);
                let (d, inside) = outline_distance(p, outline);
                let t = if inside {
                    1.0
                } else if margin > 0.0 && d < margin {
                    1.0 - smoothstep(d / margin)
                } else {
                    continue;
                };
                let i = self.at(ix, iz);
                self.heights[i] = f(self.heights[i], t);
            }
        }
        r
    }

    /// Makes the grid `width` by `depth` cells at `origin`, keeping the data where it overlaps the
    /// old grid (same cell size; vertices are matched to the nearest old one). New ground is flat at
    /// height 0 in the base material.
    pub fn resized(
        &self,
        origin: [f32; 2],
        width_m: f32,
        depth_m: f32,
    ) -> Result<Self, TerrainError> {
        let mut new = Self::flat(
            origin,
            width_m,
            depth_m,
            self.cell_m,
            &self.layers[0].material,
        )?;
        new.layers = self.layers.clone();
        let n = self.layers.len();
        new.weights = (0..new.vertex_count())
            .flat_map(|_| (0..n).map(|l| if l == 0 { 255 } else { 0 }))
            .collect();
        for iz in 0..=new.depth {
            for ix in 0..=new.width {
                let p = new.xz(ix, iz);
                let (u, v) = (
                    (p[0] - self.origin[0]) / self.cell_m,
                    (p[1] - self.origin[1]) / self.cell_m,
                );
                let (ox, oz) = (u.round(), v.round());
                if ox >= 0.0 && oz >= 0.0 && ox <= self.width as f32 && oz <= self.depth as f32 {
                    let (a, b) = (self.at(ox as u32, oz as u32), new.at(ix, iz));
                    new.heights[b] = self.heights[a];
                    new.weights[b * n..(b + 1) * n]
                        .copy_from_slice(&self.weights[a * n..(a + 1) * n]);
                }
            }
        }
        Ok(new)
    }

    /// Appends a layer with no weight anywhere; `None` when there are already [`MAX_LAYERS`].
    pub fn add_layer(&mut self, material: &str) -> Option<usize> {
        let n = self.layers.len();
        if n >= MAX_LAYERS {
            return None;
        }
        let mut weights = Vec::with_capacity(self.vertex_count() * (n + 1));
        for w in self.weights.chunks_exact(n) {
            weights.extend_from_slice(w);
            weights.push(0);
        }
        self.weights = weights;
        self.layers.push(TerrainLayer {
            material: material.into(),
        });
        Some(n)
    }

    /// Removes a layer, sharing its weight among the others in proportion (all of it to the first if they have none).
    /// The last layer cannot be removed.
    pub fn remove_layer(&mut self, index: usize) -> Result<(), TerrainError> {
        let n = self.layers.len();
        if n < 2 || index >= n {
            return err("no such layer, or it is the only one");
        }
        let mut weights = Vec::with_capacity(self.vertex_count() * (n - 1));
        for w in self.weights.chunks_exact(n) {
            let rest: Vec<f32> = (0..n)
                .filter(|i| *i != index)
                .map(|i| f32::from(w[i]))
                .collect();
            weights.extend(normalize255(&rest));
        }
        self.weights = weights;
        self.layers.remove(index);
        Ok(())
    }
}

fn bounds(points: &[[f32; 2]], margin: f32) -> [[f32; 2]; 2] {
    let mut b = [[f32::INFINITY; 2], [f32::NEG_INFINITY; 2]];
    for p in points {
        for k in 0..2 {
            b[0][k] = b[0][k].min(p[k] - margin);
            b[1][k] = b[1][k].max(p[k] + margin);
        }
    }
    b
}

// ---- meshes ----

/// One chunk ready for the GPU.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerrainChunk {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Counter-clockwise seen from above (and from outside, for skirts).
    pub indices: Vec<u32>,
    /// Material weights per vertex, padded with zeros to eight layers.
    pub weights: Vec<[u8; 8]>,
    /// World `(x, z)`, for texturing.
    pub uv: Vec<[f32; 2]>,
}

/// How many chunks of [`CHUNK_CELLS`] cells cover the terrain in x and z.
pub fn terrain_chunks(t: &Terrain) -> (u32, u32) {
    (t.width.div_ceil(CHUNK_CELLS), t.depth.div_ceil(CHUNK_CELLS))
}

/// Mesh of chunk `(chunk_x, chunk_z)`. `lod` is the vertex step: 1, 2, 4 or 8 (other values are
/// rounded up to a power of two and capped at 8). Normals are central differences on the full
/// grid, so they agree across chunks and LODs; a skirt hangs down every chunk edge so that two
/// neighbours at different LODs show no crack. An out-of-range chunk is empty. Same as
/// [`terrain_chunk_mesh_on`] without a ground, so every edge gets its skirt.
pub fn terrain_chunk_mesh(t: &Terrain, chunk_x: u32, chunk_z: u32, lod: u32) -> TerrainChunk {
    terrain_chunk_mesh_on(t, chunk_x, chunk_z, lod, None)
}

/// [`terrain_chunk_mesh`] for a terrain that sits in an infinite ground plane at `ground`
/// (the project's `view.ground` height): a stretch of the terrain's outer border whose edge is at
/// that height gets no skirt, so the terrain does not read as a slab against the ground. Skirts
/// between chunks (which hide cracks between LODs) are always kept, and so are outer ones where
/// the terrain stands above or below the ground.
pub fn terrain_chunk_mesh_on(
    t: &Terrain,
    chunk_x: u32,
    chunk_z: u32,
    lod: u32,
    ground: Option<f32>,
) -> TerrainChunk {
    terrain_chunk_mesh_holes(t, chunk_x, chunk_z, lod, ground, &[])
}

/// A hole's bounding box and its triangles.
type HoleBox = ([[f32; 2]; 2], Vec<[[f32; 2]; 3]>);

/// A point of a chunk: world `(x, z)` with its attributes, to be mixed by barycentric weights.
struct Corner {
    p: [f32; 3],
    n: [f32; 3],
    w: [u8; 8],
}

/// [`terrain_chunk_mesh_on`] with `holes` cut out (the footprints of terraces, which replace the
/// ground inside them). A triangle that a hole touches is clipped against it exactly: what is
/// left outside keeps the heights, normals and weights the surface had there, so the terrain meets
/// the wall that closes the hole at the original ground. Skirts are the chunk's own and stay.
pub fn terrain_chunk_mesh_holes(
    t: &Terrain,
    chunk_x: u32,
    chunk_z: u32,
    lod: u32,
    ground: Option<f32>,
    holes: &[Vec<[f32; 2]>],
) -> TerrainChunk {
    let mut m = TerrainChunk::default();
    let (cx, cz) = terrain_chunks(t);
    if chunk_x >= cx || chunk_z >= cz {
        return m;
    }
    let step = lod.max(1).next_power_of_two().min(8);
    let line = |first: u32, last: u32| -> Vec<u32> {
        let mut v: Vec<u32> = (first..=last).step_by(step as usize).collect();
        if *v.last().unwrap() != last {
            v.push(last);
        }
        v
    };
    let xs = line(
        chunk_x * CHUNK_CELLS,
        ((chunk_x + 1) * CHUNK_CELLS).min(t.width),
    );
    let zs = line(
        chunk_z * CHUNK_CELLS,
        ((chunk_z + 1) * CHUNK_CELLS).min(t.depth),
    );
    let n = t.layers.len();
    let vertex = |m: &mut TerrainChunk, ix: u32, iz: u32, drop: f32| {
        let p = t.xz(ix, iz);
        let h = |dx: i64, dz: i64| t.height_clamped(i64::from(ix) + dx, i64::from(iz) + dz);
        let (sx, sz) = (
            if ix == 0 || ix == t.width { 1.0 } else { 2.0 },
            if iz == 0 || iz == t.depth { 1.0 } else { 2.0 },
        );
        let normal = normalise([
            -(h(1, 0) - h(-1, 0)) / (sx * t.cell_m),
            1.0,
            -(h(0, 1) - h(0, -1)) / (sz * t.cell_m),
        ]);
        let mut w = [0u8; 8];
        let i = t.at(ix, iz) * n;
        w[..n].copy_from_slice(&t.weights[i..i + n]);
        m.positions
            .push([p[0], t.heights[t.at(ix, iz)] - drop, p[1]]);
        m.normals.push(normal);
        m.weights.push(w);
        m.uv.push(p);
        m.positions.len() as u32 - 1
    };
    let mut grid = Vec::with_capacity(zs.len() * xs.len());
    for &iz in &zs {
        for &ix in &xs {
            grid.push(vertex(&mut m, ix, iz, 0.0));
        }
    }
    let at = |i: usize, j: usize| grid[j * xs.len() + i];
    // The holes this chunk can see, each as its bounding box and triangles.
    let cut: Vec<HoleBox> = holes
        .iter()
        .filter(|h| h.len() >= 3)
        .map(|h| (bounds(h, 0.0), hole_triangles(h)))
        .collect();
    for j in 0..zs.len() - 1 {
        for i in 0..xs.len() - 1 {
            let (v00, v10, v01, v11) = (at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1));
            let (p0, p1) = (m.positions[v00 as usize], m.positions[v11 as usize]);
            let near: Vec<&Vec<[[f32; 2]; 3]>> = cut
                .iter()
                .filter(|(b, _)| {
                    b[0][0] < p1[0].max(p0[0])
                        && b[1][0] > p0[0].min(p1[0])
                        && b[0][1] < p1[2].max(p0[2])
                        && b[1][1] > p0[2].min(p1[2])
                })
                .map(|(_, tris)| tris)
                .collect();
            if near.is_empty() {
                m.indices.extend([v00, v01, v11, v00, v11, v10]);
                continue;
            }
            for tri in [[v00, v01, v11], [v00, v11, v10]] {
                clip_triangle(&mut m, tri, &near);
            }
        }
    }
    // Skirts: a copy of each edge vertex, lowered, joined by a strip facing outwards. Deep enough
    // to close the crack to a neighbour at any level: twice the most any level's edge strays
    // from the full grid along this edge, plus a margin. (A fixed drop of a few cells hung a
    // wall below the ground at every chunk seam on the outer border, which showed from outside
    // as a dark or sky-blue triangle.)
    let edge_drop = |ring: &[(usize, usize)]| {
        let (a, b) = (ring[0], ring[ring.len() - 1]);
        let full: Vec<f32> = if a.1 == b.1 {
            (xs[a.0]..=xs[b.0])
                .map(|ix| t.heights[t.at(ix, zs[a.1])])
                .collect()
        } else {
            (zs[a.1]..=zs[b.1])
                .map(|iz| t.heights[t.at(xs[a.0], iz)])
                .collect()
        };
        let n = full.len() - 1;
        let mut stray = 0.0f32;
        for s in [1, 2, 4, 8] {
            let mut i = 0;
            while i < n {
                let j = (i + s).min(n);
                for k in i..=j {
                    let f = (k - i) as f32 / (j - i) as f32;
                    stray = stray.max((full[k] - (full[i] + (full[j] - full[i]) * f)).abs());
                }
                i = j;
            }
        }
        2.0 * stray + 0.25
    };
    let on_border =
        |i: usize, j: usize| xs[i] == 0 || xs[i] == t.width || zs[j] == 0 || zs[j] == t.depth;
    let edge = |m: &mut TerrainChunk, ring: Vec<(usize, usize)>, flip: bool, outer: bool| {
        let drop = edge_drop(&ring);
        let top: Vec<u32> = ring.iter().map(|&(i, j)| at(i, j)).collect();
        // A stretch on the outer border at ground height needs no skirt.
        let flush = |m: &TerrainChunk, v: u32| {
            ground.is_some_and(|g| (m.positions[v as usize][1] - g).abs() <= 1e-3)
        };
        let keep: Vec<bool> = (0..top.len() - 1)
            .map(|k| !(outer && flush(m, top[k]) && flush(m, top[k + 1])))
            .collect();
        // A copy of vertex `v`, hung `drop` lower; `border` when it lies on the terrain's border.
        let lowered = |m: &mut TerrainChunk, v: u32, border: bool| -> u32 {
            let (p, nrm, w, uv) = (
                m.positions[v as usize],
                m.normals[v as usize],
                m.weights[v as usize],
                m.uv[v as usize],
            );
            // An outer skirt above the ground plane stops at it: the plane is drawn
            // behind everything, so a skirt hanging below it showed through as a dark
            // or sky-blue sliver wherever an edge vertex sat a hair off the ground.
            // Below the plane there is nothing to hide: the plane shows through.
            // So does the end of a seam's skirt on the outer border.
            let bottom = match ground {
                Some(g) if outer || border => (p[1] - drop).max(g.min(p[1])),
                _ => p[1] - drop,
            };
            m.positions.push([p[0], bottom, p[2]]);
            m.normals.push(nrm);
            m.weights.push(w);
            m.uv.push(uv);
            m.positions.len() as u32 - 1
        };
        let strip = |m: &mut TerrainChunk, a: u32, b: u32, a2: u32, b2: u32| {
            if flip {
                m.indices.extend([a, b2, b, a, a2, b2]);
            } else {
                m.indices.extend([a, b, b2, a, b2, a2]);
            }
        };
        let mut low: Vec<Option<u32>> = vec![None; top.len()];
        for k in 0..top.len() - 1 {
            if !keep[k] {
                continue;
            }
            let (a, b) = (top[k], top[k + 1]);
            // Where a terrace's hole lies over the seam there is no ground to hang a skirt
            // from: only the stretches outside the holes get one.
            let spans = if cut.is_empty() {
                vec![(0.0, 1.0)]
            } else {
                let xz = |v: u32| [m.positions[v as usize][0], m.positions[v as usize][2]];
                outside_spans(xz(a), xz(b), holes)
            };
            if spans.as_slice() != [(0.0, 1.0)] {
                for (t0, t1) in spans {
                    let ends = [t0, t1].map(|f| {
                        let v = mixed_vertex(m, a, b, f);
                        let border = on_border(ring[k].0, ring[k].1)
                            && on_border(ring[k + 1].0, ring[k + 1].1);
                        (v, lowered(m, v, border))
                    });
                    strip(m, ends[0].0, ends[1].0, ends[0].1, ends[1].1);
                }
                continue;
            }
            for e in [k, k + 1] {
                if low[e].is_none() {
                    low[e] = Some(lowered(m, top[e], on_border(ring[e].0, ring[e].1)));
                }
            }
            strip(m, a, b, low[k].unwrap(), low[k + 1].unwrap());
        }
    };
    let (nx, nz) = (xs.len() - 1, zs.len() - 1);
    let (west, east) = (xs[0] == 0, xs[nx] == t.width);
    let (north, south) = (zs[0] == 0, zs[nz] == t.depth);
    edge(&mut m, (0..=nx).map(|i| (i, 0)).collect(), false, north);
    edge(&mut m, (0..=nx).map(|i| (i, nz)).collect(), true, south);
    edge(&mut m, (0..=nz).map(|j| (0, j)).collect(), true, west);
    edge(&mut m, (0..=nz).map(|j| (nx, j)).collect(), false, east);
    m
}

/// The parts of the segment `a`-`b` (as parameter ranges 0..=1) that lie outside every polygon.
fn outside_spans(a: [f32; 2], b: [f32; 2], polys: &[Vec<[f32; 2]>]) -> Vec<(f32, f32)> {
    let mut cuts = vec![0.0f32, 1.0];
    let d = [b[0] - a[0], b[1] - a[1]];
    for poly in polys {
        let n = poly.len();
        for i in 0..n {
            let (p, q) = (poly[i], poly[(i + 1) % n]);
            let e = [q[0] - p[0], q[1] - p[1]];
            let den = d[0] * e[1] - d[1] * e[0];
            if den.abs() < 1e-12 {
                continue;
            }
            let w = [p[0] - a[0], p[1] - a[1]];
            let t = (w[0] * e[1] - w[1] * e[0]) / den;
            let u = (w[0] * d[1] - w[1] * d[0]) / den;
            if t > 0.0 && t < 1.0 && (0.0..=1.0).contains(&u) {
                cuts.push(t);
            }
        }
    }
    cuts.sort_by(f32::total_cmp);
    let mut out: Vec<(f32, f32)> = Vec::new();
    for w in cuts.windows(2) {
        let mid = (w[0] + w[1]) / 2.0;
        if w[1] - w[0] < 1e-6 {
            continue;
        }
        let at = [a[0] + d[0] * mid, a[1] + d[1] * mid];
        if polys
            .iter()
            .any(|p| crate::planting::point_in_polygon(at, p))
        {
            continue;
        }
        match out.last_mut() {
            Some(last) if (last.1 - w[0]).abs() < 1e-6 => last.1 = w[1],
            _ => out.push((w[0], w[1])),
        }
    }
    out
}

/// The vertex a fraction `f` of the way from `a` to `b` (both are vertices of `m`): `a` or `b`
/// themselves at the ends, else a new one with mixed attributes.
fn mixed_vertex(m: &mut TerrainChunk, a: u32, b: u32, f: f32) -> u32 {
    if f <= 0.0 {
        return a;
    }
    if f >= 1.0 {
        return b;
    }
    let (ia, ib) = (a as usize, b as usize);
    let lerp = |x: f32, y: f32| x + (y - x) * f;
    let p = [0, 1, 2].map(|k| lerp(m.positions[ia][k], m.positions[ib][k]));
    let n = normalise([0, 1, 2].map(|k| lerp(m.normals[ia][k], m.normals[ib][k])));
    let mut w = [0u8; 8];
    for (k, slot) in w.iter_mut().enumerate() {
        *slot = lerp(f32::from(m.weights[ia][k]), f32::from(m.weights[ib][k]))
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    let uv = [0, 1].map(|k| lerp(m.uv[ia][k], m.uv[ib][k]));
    m.positions.push(p);
    m.normals.push(n);
    m.weights.push(w);
    m.uv.push(uv);
    m.positions.len() as u32 - 1
}

/// A polygon as triangles on the x/z plane.
fn hole_triangles(poly: &[[f32; 2]]) -> Vec<[[f32; 2]; 3]> {
    let flat: Vec<f64> = poly
        .iter()
        .flat_map(|p| [f64::from(p[0]), f64::from(p[1])])
        .collect();
    earcutr::earcut(&flat, &[], 2)
        .unwrap_or_default()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.map(|i| poly[i]))
        .collect()
}

/// Adds triangle `tri` (vertices of `m`) to the chunk's indices minus the `holes` (as triangles):
/// the parts outside every hole, as new vertices on the same plane with mixed attributes.
fn clip_triangle(m: &mut TerrainChunk, tri: [u32; 3], holes: &[&Vec<[[f32; 2]; 3]>]) {
    let corners: [Corner; 3] = tri.map(|v| Corner {
        p: m.positions[v as usize],
        n: m.normals[v as usize],
        w: m.weights[v as usize],
    });
    let xz = corners.each_ref().map(|c| [c.p[0], c.p[2]]);
    let mut pieces = vec![xz.to_vec()];
    for tris in holes {
        for h in tris.iter() {
            pieces = pieces
                .into_iter()
                .flat_map(|p| crate::shape::subtract_triangle(p, h))
                .collect();
        }
    }
    // Barycentric weights of `q` in the original triangle.
    let det = (xz[1][1] - xz[2][1]) * (xz[0][0] - xz[2][0])
        + (xz[2][0] - xz[1][0]) * (xz[0][1] - xz[2][1]);
    let mix = |q: [f32; 2]| -> [f32; 3] {
        if det.abs() < 1e-12 {
            return [1.0, 0.0, 0.0];
        }
        let l0 = ((xz[1][1] - xz[2][1]) * (q[0] - xz[2][0])
            + (xz[2][0] - xz[1][0]) * (q[1] - xz[2][1]))
            / det;
        let l1 = ((xz[2][1] - xz[0][1]) * (q[0] - xz[2][0])
            + (xz[0][0] - xz[2][0]) * (q[1] - xz[2][1]))
            / det;
        [l0, l1, 1.0 - l0 - l1]
    };
    for piece in pieces {
        let first = m.positions.len() as u32;
        for q in &piece {
            let l = mix(*q);
            let blend = |f: &dyn Fn(&Corner) -> f32| {
                l[0] * f(&corners[0]) + l[1] * f(&corners[1]) + l[2] * f(&corners[2])
            };
            m.positions.push([q[0], blend(&|c| c.p[1]), q[1]]);
            m.normals.push(normalise([
                blend(&|c| c.n[0]),
                blend(&|c| c.n[1]),
                blend(&|c| c.n[2]),
            ]));
            let mut w = [0u8; 8];
            for (k, slot) in w.iter_mut().enumerate() {
                *slot = blend(&|c| f32::from(c.w[k])).round().clamp(0.0, 255.0) as u8;
            }
            m.weights.push(w);
            m.uv.push(*q);
        }
        for k in 1..piece.len().saturating_sub(1) {
            // The cuts leave slivers of rounding size along an edge: they draw nothing.
            let (a, b, c) = (piece[0], piece[k], piece[k + 1]);
            let area = ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0;
            if area > 1e-5 {
                m.indices
                    .extend([first, first + k as u32, first + k as u32 + 1]);
            }
        }
    }
}

/// A coast across the terrain (`terrain.shore`): heights relative to the water `level` by the
/// signed distance from a line, positive on its right (the sea side). From the line the land
/// drops over `bank_m` from `top_m` to `berm_m` above the water (a bank or bluff; 0 wide: none),
/// then the beach runs down over `beach_m` to the waterline, then the bed falls to `depth_m`
/// below the water over `shelf_m` along Dean's equilibrium profile (depth ~ distance^(2/3):
/// R. G. Dean, "Equilibrium beach profiles: characteristics and applications", Journal of
/// Coastal Research 7(1), 1991) and stays there. It only lowers ground: on the sea side to the
/// profile, on the land side what stands above `top_m` down to it over 6 m; raise the land first
/// for a higher bluff.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShoreProfile {
    pub level: f32,
    pub top_m: f32,
    pub bank_m: f32,
    pub berm_m: f32,
    pub beach_m: f32,
    pub shelf_m: f32,
    pub depth_m: f32,
}

/// Land blended to the line's height over this width on the land side.
const SHORE_BLEND_M: f32 = 6.0;

impl ShoreProfile {
    /// The height at signed distance `d` (positive seaward) over ground `old`.
    pub fn height(&self, d: f32, old: f32) -> f32 {
        let top = self.level + self.top_m;
        // The coast is cut out of the land: ground is only ever lowered, so a second stretch
        // of coast crossing the first neither builds a wall across its beach nor a spit into
        // its sea.
        if d <= 0.0 {
            return if old > top {
                old + (top - old) * smoothstep(1.0 + d / SHORE_BLEND_M)
            } else {
                old
            };
        }
        old.min(self.profile(d))
    }

    /// The profile's height at signed distance `d` > 0 (seaward).
    fn profile(&self, d: f32) -> f32 {
        let top = self.level + self.top_m;
        let berm = self.level + self.berm_m;
        if d < self.bank_m {
            return top + (berm - top) * smoothstep(d / self.bank_m);
        }
        let d = d - self.bank_m;
        if d < self.beach_m {
            return berm - self.berm_m * d / self.beach_m.max(1e-3);
        }
        let x = ((d - self.beach_m) / self.shelf_m.max(1e-3)).min(1.0);
        self.level - self.depth_m * x.powf(2.0 / 3.0)
    }
}

/// Signed distance from `p` to the polyline: positive to the right of the direction of travel
/// (+X east, +Z south: travelling north, the right is east). Past the ends the end segments
/// run on, so the coast does not curl round them.
pub fn signed_distance(p: [f32; 2], line: &[[f32; 2]]) -> f32 {
    let mut best = (f32::INFINITY, 0.0f32);
    let n = line.len();
    for i in 0..n.saturating_sub(1) {
        let (a, b) = (line[i], line[i + 1]);
        let ab = [b[0] - a[0], b[1] - a[1]];
        let len2 = ab[0] * ab[0] + ab[1] * ab[1];
        if len2 <= 0.0 {
            continue;
        }
        let ap = [p[0] - a[0], p[1] - a[1]];
        let mut u = (ap[0] * ab[0] + ap[1] * ab[1]) / len2;
        if i > 0 {
            u = u.max(0.0);
        }
        if i + 2 < n {
            u = u.min(1.0);
        }
        let q = [a[0] + ab[0] * u, a[1] + ab[1] * u];
        let dist = (p[0] - q[0]).hypot(p[1] - q[1]);
        if dist < best.0 {
            // The right of (dx, dz) is (-dz, dx).
            let side = ap[0] * -ab[1] + ap[1] * ab[0];
            best = (dist, if side >= 0.0 { 1.0 } else { -1.0 });
        }
    }
    best.0 * best.1
}

impl Terrain {
    /// Shapes a coast along `line` (`ShoreProfile`); with `paint`, the beach and the bed below
    /// it get that layer (fading in over the bank).
    pub fn shore(
        &mut self,
        line: &[[f32; 2]],
        p: &ShoreProfile,
        paint: Option<usize>,
    ) -> DirtyRect {
        for iz in 0..=self.depth {
            for ix in 0..=self.width {
                let d = signed_distance(self.xz(ix, iz), line);
                let i = self.at(ix, iz);
                self.heights[i] = p.height(d, self.heights[i]);
                if let Some(layer) = paint.filter(|l| *l < self.layers.len()) {
                    let t = smoothstep((d - p.bank_m * 0.6) / (p.bank_m * 0.4 + 1.0));
                    if t > 0.0 {
                        self.paint_vertex(i, layer, t);
                    }
                }
            }
        }
        self.whole()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hill() -> Terrain {
        let mut t = Terrain::flat([0.0, 0.0], 20.0, 20.0, 0.5, "lawn").unwrap();
        let s = BrushStroke {
            tool: Tool::Raise,
            points: vec![[10.0, 10.0]],
            radius_m: 6.0,
            strength: 0.5,
            falloff: 0.8,
            target_height: None,
            layer: None,
        };
        for _ in 0..10 {
            apply_stroke(&mut t, &s);
        }
        t
    }

    fn stroke(tool: Tool, points: Vec<[f32; 2]>) -> BrushStroke {
        BrushStroke {
            tool,
            points,
            radius_m: 3.0,
            strength: 0.1,
            falloff: 0.5,
            target_height: None,
            layer: None,
        }
    }

    #[test]
    fn strokes_are_deterministic() {
        for tool in [Tool::Raise, Tool::Noise, Tool::Smooth, Tool::Flatten] {
            let s = stroke(tool, vec![[3.0, 3.0], [12.0, 9.0], [15.0, 15.0]]);
            let (mut a, mut b) = (hill(), hill());
            let da = apply_stroke(&mut a, &s);
            assert_eq!(da, apply_stroke(&mut b, &s));
            assert_eq!(a, b);
            assert!(!da.is_empty());
        }
        let mut t = hill();
        assert!(apply_stroke(&mut t, &stroke(Tool::Noise, vec![[5.0, 5.0]])).x0 < 11);
        assert_ne!(t, hill());
    }

    #[test]
    fn raise_then_lower_returns_to_the_start() {
        let mut t = hill();
        let before = t.heights.clone();
        let mut s = stroke(Tool::Raise, vec![[4.0, 4.0], [14.0, 12.0]]);
        apply_stroke(&mut t, &s);
        assert_ne!(t.heights, before);
        s.tool = Tool::Lower;
        apply_stroke(&mut t, &s);
        assert!(
            t.heights
                .iter()
                .zip(&before)
                .all(|(a, b)| (a - b).abs() < 1e-4)
        );
    }

    #[test]
    fn flatten_converges_and_smooth_reduces_roughness() {
        let mut t = hill();
        let mut s = stroke(Tool::Flatten, vec![[10.0, 10.0]]);
        s.radius_m = 8.0;
        s.falloff = 0.0;
        s.target_height = Some(1.0);
        s.strength = 0.5;
        for _ in 0..40 {
            apply_stroke(&mut t, &s);
        }
        assert!((t.height_at(10.0, 10.0).unwrap() - 1.0).abs() < 1e-3);
        assert!((t.height_at(12.0, 8.0).unwrap() - 1.0).abs() < 1e-3);
        let mut n = hill();
        apply_stroke(&mut n, &stroke(Tool::Noise, vec![[10.0, 10.0]]));
        let rough = |t: &Terrain| {
            t.heights
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .sum::<f32>()
        };
        let r0 = rough(&n);
        let mut sm = stroke(Tool::Smooth, vec![[10.0, 10.0]]);
        sm.strength = 0.8;
        apply_stroke(&mut n, &sm);
        assert!(rough(&n) < r0);
    }

    #[test]
    fn paint_keeps_every_vertex_at_255() {
        let mut t = hill();
        let (a, b) = (t.add_layer("gravel").unwrap(), t.add_layer("soil").unwrap());
        let mut s = stroke(Tool::Paint, vec![[5.0, 5.0], [14.0, 14.0]]);
        s.strength = 0.4;
        s.layer = Some(a);
        apply_stroke(&mut t, &s);
        s.layer = Some(b);
        s.points = vec![[8.0, 12.0]];
        apply_stroke(&mut t, &s);
        s.layer = Some(0);
        apply_stroke(&mut t, &s);
        assert!(t.validate().is_ok());
        let w = t.weights_at(5.0, 5.0).unwrap();
        assert!(w[1] > 100, "{w:?}");
        t.remove_layer(a).unwrap();
        assert!(t.validate().is_ok() && t.layers.len() == 2);
        assert!(apply_stroke(&mut t, &stroke(Tool::Paint, vec![[1.0, 1.0]])).is_empty());
    }

    #[test]
    fn height_at_is_bilinear_on_a_slope() {
        let mut t = Terrain::flat([-2.0, 4.0], 10.0, 10.0, 1.0, "lawn").unwrap();
        for iz in 0..=t.depth {
            for ix in 0..=t.width {
                let i = t.at(ix, iz);
                t.heights[i] = 0.5 * ix as f32 + 0.25 * iz as f32;
            }
        }
        let h = t.height_at(1.3, 6.6).unwrap();
        assert!((h - (0.5 * 3.3 + 0.25 * 2.6)).abs() < 1e-5, "{h}");
        assert_eq!(t.height_at(-2.1, 5.0), None);
        assert_eq!(t.height_at(8.01, 5.0), None);
        assert!(t.height_at(8.0, 14.0).is_some());
        let n = t.normal_at(3.0, 8.0);
        assert!(
            (n[0] + 0.5 / (0.5f32.powi(2) + 0.25f32.powi(2) + 1.0).sqrt()).abs() < 1e-4,
            "{n:?}"
        );
    }

    #[test]
    fn raycast_hits_the_hill_and_misses_the_sky() {
        let t = hill();
        let top = t.height_at(10.0, 10.0).unwrap();
        let (d, p, n) = t.raycast([10.0, 50.0, 10.0], [0.0, -1.0, 0.0]).unwrap();
        assert!((p[1] - top).abs() < 0.3 && (d - (50.0 - p[1])).abs() < 1e-3 && n[1] > 0.9);
        // A shallow ray from the side hits the flank before the top.
        let (_, p, _) = t
            .raycast([-5.0, top + 1.0, 10.0], [1.0, -0.2, 0.0])
            .unwrap();
        assert!(p[0] > 3.0 && p[0] < 10.0, "{p:?}");
        assert!(t.raycast([10.0, 50.0, 10.0], [0.0, 1.0, 0.0]).is_none());
        assert!(t.raycast([-5.0, 50.0, -5.0], [0.0, -1.0, 0.0]).is_none());
        assert!(t.raycast([-5.0, 3.0, 10.0], [1.0, 1.0, 0.0]).is_none());
    }

    #[test]
    fn level_and_carve() {
        let outline = [[6.0, 6.0], [14.0, 6.0], [14.0, 14.0], [6.0, 14.0]];
        let mut t = hill();
        let before = t.clone();
        t.level_under(&outline, 0.7, 2.0);
        for (x, z) in [(6.0, 6.0), (10.0, 10.0), (13.5, 7.0)] {
            assert_eq!(t.height_at(x, z), Some(0.7));
        }
        assert_eq!(t.height_at(1.0, 1.0), before.height_at(1.0, 1.0));
        let mid = t.height_at(15.0, 10.0).unwrap();
        let old = before.height_at(15.0, 10.0).unwrap();
        assert!(
            mid > old.min(0.7) - 1e-4 && mid < old.max(0.7) + 1e-4 && (mid - old).abs() > 1e-4
                || (old - 0.7).abs() < 1e-3
        );
        let mut f = Terrain::flat([0.0, 0.0], 20.0, 20.0, 0.5, "lawn").unwrap();
        f.carve(&outline, 1.2, 3.0);
        assert!((f.height_at(10.0, 10.0).unwrap() + 1.2).abs() < 1e-6);
        let bank = f.height_at(15.5, 10.0).unwrap();
        assert!(bank < 0.0 && bank > -1.2);
        assert_eq!(f.height_at(18.0, 10.0), Some(0.0));
    }

    #[test]
    fn snapshots_share_until_edited() {
        use std::sync::Arc;
        let a = Arc::new(hill());
        let mut b = Arc::clone(&a);
        assert!(Arc::ptr_eq(&a, &b));
        apply_stroke(
            Arc::make_mut(&mut b),
            &stroke(Tool::Raise, vec![[5.0, 5.0]]),
        );
        assert!(!Arc::ptr_eq(&a, &b) && a != b);
    }

    #[test]
    fn json_round_trip_is_exact_and_compact() {
        let mut t = Terrain::flat([-10.0, 5.0], 128.0, 128.0, 0.5, "lawn").unwrap();
        assert_eq!((t.width, t.depth), (256, 256));
        t.add_layer("gravel");
        for (k, p) in [[40.0, 30.0], [90.0, 60.0], [30.0, 100.0]]
            .into_iter()
            .enumerate()
        {
            let mut s = stroke(Tool::Raise, vec![p, [p[0] + 30.0, p[1] + 10.0]]);
            s.radius_m = 15.0;
            s.strength = 0.05;
            apply_stroke(&mut t, &s);
            s.tool = Tool::Noise;
            apply_stroke(&mut t, &s);
            s.tool = Tool::Paint;
            s.layer = Some(1);
            s.points = vec![[20.0 * k as f32 + 40.0, 50.0]];
            apply_stroke(&mut t, &s);
        }
        let json = serde_json::to_string(&t).unwrap();
        let raw = (t.heights.len() * 4 + t.weights.len()) as f32;
        eprintln!(
            "terrain 257x257 vertices: {} bytes of JSON for {raw} raw bytes",
            json.len()
        );
        let back: Terrain = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
        assert!(json.len() < 600_000);
        assert!(
            serde_json::from_str::<Terrain>(&json.replace("\"width\":256", "\"width\":255"))
                .is_err()
        );
    }

    #[test]
    fn chunk_meshes_match_at_the_borders() {
        let mut t = Terrain::flat([0.0, 0.0], 100.0, 70.0, 0.5, "lawn").unwrap();
        let mut s = stroke(Tool::Noise, vec![[10.0, 10.0], [80.0, 50.0]]);
        s.radius_m = 20.0;
        s.strength = 0.5;
        apply_stroke(&mut t, &s);
        assert_eq!(terrain_chunks(&t), (4, 3));
        let a = terrain_chunk_mesh(&t, 1, 1, 1);
        let b = terrain_chunk_mesh(&t, 1, 1, 8);
        assert!(b.positions.len() < a.positions.len() / 20);
        let extent = |m: &TerrainChunk| {
            let top: Vec<_> = m.positions.iter().filter(|p| p[1] > -100.0).collect();
            (
                top.iter().map(|p| p[0]).fold(f32::MAX, f32::min),
                top.iter().map(|p| p[0]).fold(f32::MIN, f32::max),
                top.iter().map(|p| p[2]).fold(f32::MAX, f32::min),
                top.iter().map(|p| p[2]).fold(f32::MIN, f32::max),
            )
        };
        assert_eq!(extent(&a), (32.0, 64.0, 32.0, 64.0));
        assert_eq!(extent(&a), extent(&b));
        // Corners agree exactly, and the coarse mesh's skirt reaches below the lowest edge height.
        let corner = |m: &TerrainChunk, x: f32, z: f32| {
            m.positions
                .iter()
                .position(|p| p[0] == x && p[2] == z)
                .map(|i| (m.positions[i][1], m.normals[i]))
        };
        for (x, z) in [(32.0, 32.0), (64.0, 64.0), (32.0, 64.0)] {
            assert_eq!(corner(&a, x, z), corner(&b, x, z));
        }
        // The lowest height of the fine surface along the edge x = 32 (grid column 64).
        let edge_low = (64..=128)
            .map(|iz| t.heights[t.at(64, iz)])
            .fold(f32::MAX, f32::min);
        assert!(
            b.positions
                .iter()
                .any(|p| p[0] == 32.0 && p[1] < edge_low - 0.2)
        );
        assert!(
            b.indices.iter().all(|&i| (i as usize) < b.positions.len())
                && b.indices.len().is_multiple_of(3)
        );
        // Upward-facing top triangles.
        let up = a.indices.chunks(3).take(100).all(|c| {
            let [p, q, r] = [c[0], c[1], c[2]].map(|i| a.positions[i as usize]);
            cross(sub(q, p), sub(r, p))[1] > 0.0
        });
        assert!(up);
        assert!(terrain_chunk_mesh(&t, 9, 0, 1).positions.is_empty());
    }

    #[test]
    fn outer_skirts_at_ground_height_are_dropped() {
        let mut t = Terrain::flat([0.0, 0.0], 100.0, 40.0, 0.5, "lawn").unwrap();
        // Raise a hill touching the west border only.
        let mut s = stroke(Tool::Raise, vec![[0.0, 20.0]]);
        s.radius_m = 6.0;
        s.strength = 2.0;
        s.falloff = 0.0;
        apply_stroke(&mut t, &s);
        let skirt_vertices = |m: &TerrainChunk| m.positions.iter().filter(|p| p[1] < -0.2).count();
        let all = terrain_chunk_mesh(&t, 0, 0, 1);
        let on_ground = terrain_chunk_mesh_on(&t, 0, 0, 1, Some(0.0));
        // The west edge crosses the hill, so its skirt stays; north and south (flat, at 0) go; the
        // east edge of chunk (0, 0) is a border with chunk (1, 0) and stays.
        assert!(skirt_vertices(&on_ground) < skirt_vertices(&all));
        assert!(skirt_vertices(&on_ground) > 0);
        assert_eq!(terrain_chunk_mesh_on(&t, 0, 0, 1, None), all);
        // Chunk (3, 0) is the north-east corner: its west and south sides border other chunks
        // (kept), its north and east sides are outer and flat (dropped).
        let corner = terrain_chunk_mesh_on(&t, 3, 0, 1, Some(0.0));
        let low: Vec<_> = corner.positions.iter().filter(|p| p[1] < -0.2).collect();
        assert!(!low.is_empty() && low.iter().all(|p| p[0] == 96.0 || p[2] == 32.0));
        // The seams' skirts end at the ground on the outer border (below it they showed from
        // outside), and over flat ground they hang only a hand deep.
        assert!(
            corner
                .positions
                .iter()
                .filter(|p| p[2] == 0.0 || p[0] == 100.0)
                .all(|p| p[1] >= 0.0)
        );
        assert!(corner.positions.iter().all(|p| p[1] > -0.3));
        // Terrain above the ground keeps every skirt, the outer ones reaching down to the ground
        // and no further (the ground is drawn behind everything: a skirt below it shows).
        let above = terrain_chunk_mesh_on(&t, 3, 0, 1, Some(-1.0));
        assert_eq!(above.indices, terrain_chunk_mesh(&t, 3, 0, 1).indices);
        assert!(
            above
                .positions
                .iter()
                .all(|p| p[1] >= -1.0 || p[0] == 96.0 || p[2] == 32.0)
        );
    }

    #[test]
    fn changed_since_finds_the_edited_vertices() {
        let before = hill();
        let mut after = before.clone();
        assert_eq!(after.changed_since(&before), None);
        let d = apply_stroke(&mut after, &stroke(Tool::Raise, vec![[3.0, 3.0]]));
        let c = after.changed_since(&before).unwrap();
        assert!(c.x0 >= d.x0 && c.x1 <= d.x1 && c.z0 >= d.z0 && c.z1 <= d.z1);
        after.add_layer("gravel");
        assert_eq!(after.changed_since(&before), Some(after.whole()));
    }

    #[test]
    fn resize_keeps_the_overlap_and_remove_layer_redistributes() {
        let t = hill();
        let bigger = t.resized([-5.0, -5.0], 30.0, 30.0).unwrap();
        assert_eq!(bigger.height_at(10.0, 10.0), t.height_at(10.0, 10.0));
        assert_eq!(bigger.height_at(-4.0, -4.0), Some(0.0));
        assert!(bigger.validate().is_ok());
        let ground = ground_height_over(Some(&t), &[], 10.0, 10.0);
        assert_eq!(ground, t.height_at(10.0, 10.0));
        assert_eq!(ground_height_over(Some(&t), &[], 100.0, 10.0), None);
    }

    /// The x/z area of every triangle with a face looking up or down (skirts stand on edge).
    fn plan_area(m: &TerrainChunk, tri: &[u32]) -> f32 {
        let p = |i: u32| m.positions[i as usize];
        let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
        ((b[0] - a[0]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[0] - a[0])).abs() / 2.0
    }

    #[test]
    fn a_triangle_is_clipped_exactly_against_a_polygon_that_is_not_convex() {
        let mut m = TerrainChunk::default();
        for (x, z, y) in [(0.0, 0.0, 0.0), (10.0, 0.0, 5.0), (0.0, 10.0, 10.0)] {
            m.positions.push([x, y, z]);
            m.normals.push([0.0, 1.0, 0.0]);
            m.weights.push([255, 0, 0, 0, 0, 0, 0, 0]);
            m.uv.push([x, z]);
        }
        // An L: 4 x 4 less a 2 x 2 corner = 12 m2, wholly inside the triangle (x + z < 10).
        let l = vec![
            [1.0, 1.0],
            [5.0, 1.0],
            [5.0, 3.0],
            [3.0, 3.0],
            [3.0, 5.0],
            [1.0, 5.0],
        ];
        let tris = hole_triangles(&l);
        clip_triangle(&mut m, [0, 1, 2], &[&tris]);
        let n = m.indices.len() / 3;
        let area: f32 = m.indices.chunks(3).map(|t| plan_area(&m, t)).sum();
        assert!((area - (50.0 - 12.0)).abs() < 1e-3, "{area}");
        for t in m.indices.chunks(3) {
            let c = t.iter().fold([0.0; 2], |c, &i| {
                [
                    c[0] + m.positions[i as usize][0] / 3.0,
                    c[1] + m.positions[i as usize][2] / 3.0,
                ]
            });
            assert!(!crate::planting::point_in_polygon(c, &l), "{c:?}");
        }
        assert!(n > 3);
        // The height is the plane's wherever a vertex lands: y = 0.5 x + z.
        for p in &m.positions[3..] {
            assert!((p[1] - (0.5 * p[0] + p[2])).abs() < 1e-4, "{p:?}");
        }
        // A hole clear of the triangle leaves it whole.
        let mut m2 = TerrainChunk {
            positions: m.positions[..3].to_vec(),
            normals: m.normals[..3].to_vec(),
            weights: m.weights[..3].to_vec(),
            uv: m.uv[..3].to_vec(),
            ..TerrainChunk::default()
        };
        let far = hole_triangles(&[[20.0, 20.0], [22.0, 20.0], [22.0, 22.0]]);
        clip_triangle(&mut m2, [0, 1, 2], &[&far]);
        assert_eq!(m2.indices.len(), 3);
    }

    #[test]
    fn a_chunk_with_a_hole_has_nothing_inside_it_and_the_rest_keeps_its_heights() {
        let mut t = Terrain::flat([0.0, 0.0], 40.0, 40.0, 0.5, "lawn").unwrap();
        let w = t.width as usize + 1;
        for (i, h) in t.heights.iter_mut().enumerate() {
            *h = 3.0 - 0.2 * (i % w) as f32 * 0.5 + 0.05 * (i / w) as f32 * 0.5;
        }
        let plane = |x: f32, z: f32| 3.0 - 0.2 * x + 0.05 * z;
        // A slanted rectangle plus a notch (not axis aligned, not convex).
        let hole = vec![
            [10.3, 8.2],
            [20.1, 9.7],
            [19.4, 14.3],
            [15.2, 13.1],
            [14.1, 18.6],
            [9.2, 17.4],
        ];
        let hole_area = crate::measure::polygon_area(&hole);
        for lod in [1, 2, 4] {
            let m = terrain_chunk_mesh_holes(&t, 0, 0, lod, None, std::slice::from_ref(&hole));
            let whole = terrain_chunk_mesh(&t, 0, 0, lod);
            let (mut flat, mut whole_flat) = (0.0, 0.0);
            for tri in m.indices.chunks(3) {
                let a = plan_area(&m, tri);
                flat += a;
                let c = tri.iter().fold([0.0; 2], |c, &i| {
                    [
                        c[0] + m.positions[i as usize][0] / 3.0,
                        c[1] + m.positions[i as usize][2] / 3.0,
                    ]
                });
                if a > 1e-4 {
                    assert!(
                        !crate::planting::point_in_polygon(c, &hole),
                        "lod {lod}: {c:?} {:?}",
                        tri.iter()
                            .map(|&i| m.positions[i as usize])
                            .collect::<Vec<_>>()
                    );
                    // Every corner is on the surface the full mesh draws.
                    for &i in tri {
                        let p = m.positions[i as usize];
                        assert!((p[1] - plane(p[0], p[2])).abs() < 1e-3, "{p:?}");
                    }
                }
            }
            for tri in whole.indices.chunks(3) {
                whole_flat += plan_area(&whole, tri);
            }
            // The chunk covers 32 x 32 m; the hole is cut from it.
            assert!((whole_flat - 1024.0).abs() < 1.0, "{whole_flat}");
            assert!(
                (flat - (whole_flat - hole_area)).abs() < 0.05,
                "{lod}: {flat}"
            );
        }
        // No hole: the same mesh as before.
        assert_eq!(
            terrain_chunk_mesh_holes(&t, 0, 0, 1, None, &[]),
            terrain_chunk_mesh(&t, 0, 0, 1)
        );
        // A hole in another chunk changes nothing here.
        let far = vec![[35.0, 35.0], [38.0, 35.0], [38.0, 38.0]];
        let a = terrain_chunk_mesh_holes(&t, 0, 0, 1, None, &[far]);
        assert_eq!(a, terrain_chunk_mesh(&t, 0, 0, 1));
    }

    #[test]
    fn a_skirt_at_a_chunk_seam_does_not_hang_in_a_hole() {
        let t = Terrain::flat([0.0, 0.0], 100.0, 30.0, 0.5, "lawn").unwrap();
        assert_eq!(terrain_chunks(&t), (4, 1));
        // 10 x 8 m across the seam at x = 32 (and x = 64 far away).
        let hole = vec![[28.0, 10.0], [38.0, 10.0], [38.0, 18.0], [28.0, 18.0]];
        for lod in [1, 4, 8] {
            for chunk in [0, 1] {
                let m =
                    terrain_chunk_mesh_holes(&t, chunk, 0, lod, None, std::slice::from_ref(&hole));
                let mut seam_skirt = 0;
                for tri in m.indices.chunks(3) {
                    let c = tri.iter().fold([0.0; 2], |c, &i| {
                        [
                            c[0] + m.positions[i as usize][0] / 3.0,
                            c[1] + m.positions[i as usize][2] / 3.0,
                        ]
                    });
                    assert!(
                        !crate::planting::point_in_polygon(c, &hole),
                        "lod {lod} chunk {chunk}: {c:?}"
                    );
                    if plan_area(&m, tri) < 1e-6 && (c[0] - 32.0).abs() < 1e-3 {
                        seam_skirt += 1;
                    }
                }
                // The seam still has its skirt, outside the hole.
                assert!(seam_skirt > 0, "lod {lod} chunk {chunk}");
            }
        }
        // Skirt vertices outside the hole keep their positions on the seam.
        let m = terrain_chunk_mesh_holes(&t, 0, 0, 1, None, std::slice::from_ref(&hole));
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for p in &m.positions {
            if (p[0] - 32.0).abs() < 1e-4 && p[1] < -0.2 {
                lo = lo.min(p[2]);
                hi = hi.max(p[2]);
            }
        }
        assert!(lo <= 0.5 && hi >= 29.5, "{lo} {hi}");
    }
}
