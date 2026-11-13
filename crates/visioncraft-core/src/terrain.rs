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
            if let Some(hit) = self.hit_cell(ix as u32, iz as u32, origin, dir) {
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
fn value_noise(seed: u64, x: f32, z: f32) -> f32 {
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
    for j in 0..zs.len() - 1 {
        for i in 0..xs.len() - 1 {
            let (v00, v10, v01, v11) = (at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1));
            m.indices.extend([v00, v01, v11, v00, v11, v10]);
        }
    }
    // Skirts: a copy of each edge vertex, lowered, joined by a strip facing outwards.
    let drop = step as f32 * t.cell_m + 0.25;
    let edge = |m: &mut TerrainChunk, ring: Vec<(usize, usize)>, flip: bool, outer: bool| {
        let top: Vec<u32> = ring.iter().map(|&(i, j)| at(i, j)).collect();
        // A stretch on the outer border at ground height needs no skirt.
        let flush = |m: &TerrainChunk, v: u32| {
            ground.is_some_and(|g| (m.positions[v as usize][1] - g).abs() <= 1e-3)
        };
        let keep: Vec<bool> = (0..top.len() - 1)
            .map(|k| !(outer && flush(m, top[k]) && flush(m, top[k + 1])))
            .collect();
        let mut low: Vec<Option<u32>> = vec![None; top.len()];
        for k in 0..top.len() - 1 {
            if !keep[k] {
                continue;
            }
            for e in [k, k + 1] {
                if low[e].is_none() {
                    let v = top[e];
                    let (p, nrm, w, uv) = (
                        m.positions[v as usize],
                        m.normals[v as usize],
                        m.weights[v as usize],
                        m.uv[v as usize],
                    );
                    m.positions.push([p[0], p[1] - drop, p[2]]);
                    m.normals.push(nrm);
                    m.weights.push(w);
                    m.uv.push(uv);
                    low[e] = Some(m.positions.len() as u32 - 1);
                }
            }
            let (a, b) = (top[k], top[k + 1]);
            let (a2, b2) = (low[k].unwrap(), low[k + 1].unwrap());
            if flip {
                m.indices.extend([a, b2, b, a, a2, b2]);
            } else {
                m.indices.extend([a, b, b2, a, b2, a2]);
            }
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
        let edge_low = a
            .positions
            .iter()
            .filter(|p| p[0] == 32.0)
            .map(|p| p[1])
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
        // Terrain above the ground keeps every skirt.
        assert_eq!(
            terrain_chunk_mesh_on(&t, 3, 0, 1, Some(-1.0)),
            terrain_chunk_mesh(&t, 3, 0, 1)
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
}
