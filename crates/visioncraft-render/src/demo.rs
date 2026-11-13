//! Built-in scenes made in code: the reference-image tests, the app's test
//! bed and the bench all use these. Textures and plants are generated here
//! too (test stand-ins: the real plant generator is `visioncraft-flora`).

use glam::{Mat4, Quat, Vec2, Vec3};

use crate::{
    AlphaMode, ColorSpace, Ground, Light, LightKind, LineVertex, Material, MaterialId, Mesh,
    MeshId, Projection, Renderer, Sun, Surface, TextureDesc, TextureId,
};

/// An owned mesh under construction. Every box face and every sphere gets
/// its own face id, numbered in the order added.
#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub face_ids: Vec<u32>,
    pub uvs: Vec<[f32; 2]>,
    pub wind: Vec<[f32; 2]>,
    next_face: u32,
}

impl MeshData {
    pub fn mesh(&self) -> Mesh<'_> {
        Mesh {
            positions: &self.positions,
            normals: &self.normals,
            indices: &self.indices,
            face_ids: Some(&self.face_ids),
            uvs: Some(&self.uvs),
            wind: Some(&self.wind),
        }
    }

    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    /// The face id the next added face will get.
    pub fn next_face(&self) -> u32 {
        self.next_face
    }

    fn vertex_full(&mut self, p: Vec3, n: Vec3, uv: Vec2, wind: [f32; 2]) -> u32 {
        self.positions.push(p.to_array());
        self.normals.push(n.to_array());
        self.face_ids.push(self.next_face);
        self.uvs.push(uv.to_array());
        self.wind.push(wind);
        self.positions.len() as u32 - 1
    }

    /// Axis-aligned box; faces in the order +X, -X, +Y, -Y, +Z, -Z. UVs run
    /// 0..1 over each face.
    pub fn add_box(&mut self, min: Vec3, max: Vec3) -> &mut Self {
        let (center, size) = ((min + max) * 0.5, max - min);
        // (normal, u, v) with u x v = normal: counter-clockwise from outside.
        let faces = [
            (Vec3::X, Vec3::Y, Vec3::Z),
            (Vec3::NEG_X, Vec3::Z, Vec3::Y),
            (Vec3::Y, Vec3::Z, Vec3::X),
            (Vec3::NEG_Y, Vec3::X, Vec3::Z),
            (Vec3::Z, Vec3::X, Vec3::Y),
            (Vec3::NEG_Z, Vec3::Y, Vec3::X),
        ];
        for (n, u, v) in faces {
            let corner = |a: f32, b: f32| center + (n * 0.5 + u * a + v * b) * size;
            let first = self.positions.len() as u32;
            for (a, b) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
                self.vertex_full(corner(a, b), n, Vec2::new(a + 0.5, 0.5 - b), [0.0; 2]);
            }
            self.indices.extend([0, 1, 2, 0, 2, 3].map(|i| first + i));
            self.next_face += 1;
        }
        self
    }

    /// UV sphere with 2 * rings * segments triangles (degenerate at poles).
    pub fn add_sphere(
        &mut self,
        center: Vec3,
        radius: f32,
        segments: u32,
        rings: u32,
    ) -> &mut Self {
        let first = self.positions.len() as u32;
        for i in 0..=rings {
            let theta = std::f32::consts::PI * i as f32 / rings as f32;
            for j in 0..=segments {
                let phi = 2.0 * std::f32::consts::PI * j as f32 / segments as f32;
                let n = Vec3::new(
                    theta.sin() * phi.cos(),
                    theta.cos(),
                    theta.sin() * phi.sin(),
                );
                let uv = Vec2::new(j as f32 / segments as f32, i as f32 / rings as f32);
                self.vertex_full(center + n * radius, n, uv, [0.0; 2]);
            }
        }
        let row = segments + 1;
        for i in 0..rings {
            for j in 0..segments {
                let a = first + i * row + j;
                // Counter-clockwise from outside: +phi before +theta.
                self.indices
                    .extend([a, a + 1, a + row, a + 1, a + row + 1, a + row]);
            }
        }
        self.next_face += 1;
        self
    }

    /// A flat quad (counter-clockwise from the side `normal` points to),
    /// UVs 0..1 from corner a (top-left) to c.
    pub fn add_quad(&mut self, corners: [Vec3; 4], wind: [[f32; 2]; 4]) -> &mut Self {
        let n = (corners[1] - corners[0])
            .cross(corners[2] - corners[0])
            .normalize_or(Vec3::Y);
        let first = self.positions.len() as u32;
        let uvs = [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y];
        for i in 0..4 {
            self.vertex_full(corners[i], n, uvs[i], wind[i]);
        }
        self.indices.extend([0, 1, 2, 0, 2, 3].map(|i| first + i));
        self.next_face += 1;
        self
    }

    /// A tapered tube along +Y from `base` (open ends), `rings` segments
    /// high; UV u around, v along (one repeat per circumference).
    pub fn add_tube(
        &mut self,
        base: Vec3,
        height: f32,
        radii: (f32, f32),
        segments: u32,
        rings: u32,
        wind: (f32, f32),
    ) -> &mut Self {
        let first = self.positions.len() as u32;
        for i in 0..=rings {
            let t = i as f32 / rings as f32;
            let r = radii.0 + (radii.1 - radii.0) * t;
            for j in 0..=segments {
                let a = std::f32::consts::TAU * j as f32 / segments as f32;
                let n = Vec3::new(a.cos(), 0.0, -a.sin());
                let uv = Vec2::new(j as f32 / segments as f32, -t * height / (6.3 * radii.0));
                let w = wind.0 + (wind.1 - wind.0) * t;
                self.vertex_full(base + n * r + Vec3::Y * height * t, n, uv, [w, 0.0]);
            }
        }
        let row = segments + 1;
        for i in 0..rings {
            for j in 0..segments {
                let a = first + i * row + j;
                self.indices
                    .extend([a, a + 1, a + row + 1, a, a + row + 1, a + row]);
            }
        }
        self.next_face += 1;
        self
    }

    /// Height field over a square, `n` x `n` quads; UVs in metres.
    pub fn add_terrain(
        &mut self,
        half: f32,
        n: u32,
        height: impl Fn(f32, f32) -> f32,
    ) -> &mut Self {
        let first = self.positions.len() as u32;
        let step = 2.0 * half / n as f32;
        for i in 0..=n {
            for j in 0..=n {
                let (x, z) = (-half + j as f32 * step, -half + i as f32 * step);
                let e = 0.05;
                let n = Vec3::new(
                    height(x - e, z) - height(x + e, z),
                    2.0 * e,
                    height(x, z - e) - height(x, z + e),
                )
                .normalize();
                self.vertex_full(Vec3::new(x, height(x, z), z), n, Vec2::new(x, z), [0.0; 2]);
            }
        }
        let row = n + 1;
        for i in 0..n {
            for j in 0..n {
                let a = first + i * row + j;
                self.indices
                    .extend([a, a + row, a + 1, a + 1, a + row, a + row + 1]);
            }
        }
        self.next_face += 1;
        self
    }
}

/// Deterministic xorshift for the generators.
#[derive(Clone)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(seed.max(1))
    }

    pub fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32)
        .wrapping_mul(0x8da6_b343)
        .wrapping_add((y as u32).wrapping_mul(0xd816_3841))
        .wrapping_add(seed.wrapping_mul(0xcb1a_b31f));
    h ^= h >> 13;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h >> 8) as f32 / 16_777_216.0
}

/// Tileable value noise (period `period` cells) on a pixel grid.
fn tile_noise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - xi as f32, y - yi as f32);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let h = |a: i32, b: i32| hash(a.rem_euclid(period), b.rem_euclid(period), seed);
    let top = h(xi, yi) + (h(xi + 1, yi) - h(xi, yi)) * s(fx);
    let bottom = h(xi, yi + 1) + (h(xi + 1, yi + 1) - h(xi, yi + 1)) * s(fx);
    top + (bottom - top) * s(fy)
}

/// An RGBA8 image made here.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    fn from_fn(width: u32, height: u32, f: impl Fn(u32, u32) -> [f32; 4]) -> Self {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                rgba.extend(f(x, y).map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8));
            }
        }
        Self {
            width,
            height,
            rgba,
        }
    }

    pub fn upload(&self, r: &mut Renderer, color_space: ColorSpace, repeat: bool) -> TextureId {
        r.add_texture(&TextureDesc {
            width: self.width,
            height: self.height,
            rgba8: &self.rgba,
            color_space,
            repeat,
        })
        .expect("demo textures are valid")
    }

    /// Tangent-space normal map (OpenGL: green up) from this image's red
    /// channel as a height field (`strength`: height per pixel step).
    fn normal_from_height(&self, strength: f32) -> Image {
        let (w, h) = (self.width as i32, self.height as i32);
        let at = |x: i32, y: i32| {
            self.rgba[((y.rem_euclid(h) * w + x.rem_euclid(w)) * 4) as usize] as f32 / 255.0
        };
        Image::from_fn(self.width, self.height, |x, y| {
            let (x, y) = (x as i32, y as i32);
            let dx = (at(x + 1, y) - at(x - 1, y)) * 0.5 * strength;
            // Image rows run down; tangent-space y runs up.
            let dy_up = (at(x, y - 1) - at(x, y + 1)) * 0.5 * strength;
            let n = Vec3::new(-dx, -dy_up, 1.0).normalize();
            [n.x * 0.5 + 0.5, n.y * 0.5 + 0.5, n.z * 0.5 + 0.5, 1.0]
        })
    }
}

/// Brick wall textures for a 1 m tile: 4 bricks across, 16 courses, running
/// bond. Returns (sRGB albedo, linear normal map, linear packed ORM).
pub fn brick_textures(size: u32) -> (Image, Image, Image) {
    let courses = 16.0;
    let across = 4.0;
    let mortar = 0.012; // tile units (1.2 cm)
    let s = size as f32;
    let brick = |x: u32, y: u32| {
        let (u, v) = ((x as f32 + 0.5) / s, (y as f32 + 0.5) / s);
        let row = (v * courses).floor();
        let shifted = u * across + if row as i32 % 2 == 1 { 0.5 } else { 0.0 };
        let col = shifted.floor();
        let (fu, fv) = (shifted.fract() / across, (v * courses).fract() / courses);
        // Distance into the brick from its nearest edge, in tile units.
        let edge = fu.min(1.0 / across - fu).min(fv.min(1.0 / courses - fv));
        (row as i32, col as i32 % across as i32, edge)
    };
    let height = Image::from_fn(size, size, |x, y| {
        let (_, _, edge) = brick(x, y);
        let bevel = ((edge - mortar * 0.5) / (mortar * 0.8)).clamp(0.0, 1.0);
        let grain = tile_noise(x as f32 / 8.0, y as f32 / 8.0, (size / 8) as i32, 7) * 0.15;
        let h = bevel * (0.85 + grain);
        [h, h, h, 1.0]
    });
    let albedo = Image::from_fn(size, size, |x, y| {
        let (row, col, edge) = brick(x, y);
        let n = tile_noise(x as f32 / 6.0, y as f32 / 6.0, (size / 6) as i32, 3);
        if edge < mortar * 0.5 {
            let g = 0.55 + 0.1 * n;
            return [g, g * 0.97, g * 0.92, 1.0];
        }
        let k = hash(row, col.rem_euclid(4), 11);
        let base = Vec3::new(0.45, 0.17, 0.10).lerp(Vec3::new(0.58, 0.30, 0.18), k);
        let c = base * (0.85 + 0.3 * n);
        [c.x, c.y, c.z, 1.0]
    });
    let normal = height.normal_from_height(6.0);
    let orm = Image::from_fn(size, size, |x, y| {
        let (_, _, edge) = brick(x, y);
        let in_mortar = edge < mortar * 0.5;
        let ao = if in_mortar { 0.65 } else { 1.0 };
        let rough = if in_mortar { 0.95 } else { 0.82 };
        [ao, rough, 0.0, 1.0]
    });
    (albedo, normal, orm)
}

/// Paving: 50 cm square slabs, light grey, with joints. (albedo, normal).
/// The paving's height (stones 1, joints 0) for a 1 m tile of two slabs.
fn paving_height(size: u32) -> Image {
    let s = size as f32;
    Image::from_fn(size, size, |x, y| {
        let (u, v) = ((x as f32 + 0.5) / s * 2.0, (y as f32 + 0.5) / s * 2.0);
        let (fu, fv) = (u.fract(), v.fract());
        let e = fu.min(1.0 - fu).min(fv.min(1.0 - fv)) / 2.0;
        let h = ((e - 0.004) / 0.008).clamp(0.0, 1.0);
        [h, h, h, 1.0]
    })
}

/// Packed ORM for the paving whose occlusion channel is its height (the
/// terrain's height blend fills the joints with the neighbouring layer).
pub fn paving_orm(size: u32) -> Image {
    let h = paving_height(size);
    Image::from_fn(size, size, |x, y| {
        let v = h.rgba[((y * size + x) * 4) as usize] as f32 / 255.0;
        [0.3 + 0.7 * v, 1.0, 0.0, 1.0]
    })
}

pub fn paving_textures(size: u32) -> (Image, Image) {
    let s = size as f32;
    let slabs = 2.0;
    let joint = 0.008;
    let edge = |x: u32, y: u32| {
        let (u, v) = ((x as f32 + 0.5) / s * slabs, (y as f32 + 0.5) / s * slabs);
        let (fu, fv) = (u.fract(), v.fract());
        (
            fu.min(1.0 - fu).min(fv.min(1.0 - fv)) / slabs,
            u.floor() as i32,
            v.floor() as i32,
        )
    };
    let height = paving_height(size);
    let albedo = Image::from_fn(size, size, |x, y| {
        let (e, a, b) = edge(x, y);
        let n = tile_noise(x as f32 / 4.0, y as f32 / 4.0, (size / 4) as i32, 5) * 0.6
            + tile_noise(x as f32 / 16.0, y as f32 / 16.0, (size / 16) as i32, 9) * 0.4;
        if e < joint * 0.5 {
            return [0.25, 0.24, 0.22, 1.0];
        }
        let k = 0.9 + 0.15 * hash(a, b, 21);
        let g = (0.42 + 0.12 * n) * k;
        [g, g * 0.98, g * 0.94, 1.0]
    });
    (albedo, height.normal_from_height(3.0))
}

/// Grass ground texture (sRGB albedo) for a 2 m tile. Large-scale
/// variation is left to `Material::variation`: a strong patch term inside a
/// 2 m tile repeats visibly and read as dark blotches across the lawn.
pub fn lawn_texture(size: u32) -> Image {
    let p = |d: u32| (size / d) as i32;
    Image::from_fn(size, size, |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        let blades = tile_noise(fx / 1.5, fy / 1.5, p(1).max(1) * 2 / 3, 1);
        let clumps = tile_noise(fx / 24.0, fy / 24.0, p(24), 2);
        let patches = tile_noise(fx / 96.0, fy / 96.0, p(96), 3);
        let c = Vec3::new(0.13, 0.24, 0.06)
            .lerp(Vec3::new(0.30, 0.40, 0.12), blades * 0.6 + clumps * 0.4)
            * (0.92 + 0.12 * patches);
        [c.x, c.y, c.z, 1.0]
    })
}

/// The lawn texture's mean colour as linear albedo (for `Ground`).
pub const LAWN_ALBEDO: [f32; 3] = [0.037, 0.083, 0.0085];

/// A leafy twig on transparent ground for foliage cards: stem up the middle,
/// alternating elliptic leaves. Colour outside the silhouette is the leaf
/// colour (alpha 0), so filtered and mipped edges do not darken.
pub fn leaf_texture(size: u32, tint: Vec3, seed: u32) -> Image {
    let s = size as f32;
    let mut rng = Rng::new(seed);
    // (centre, direction, length, width) in pixels.
    let mut leaves = Vec::new();
    for i in 0..9 {
        let t = 0.15 + 0.8 * i as f32 / 8.0;
        let side = if i % 2 == 0 { 1.0 } else { -1.0 };
        let angle = side * rng.range(0.6, 1.0) - side * t * 0.3;
        let dir = Vec2::new(angle.sin(), -angle.cos());
        let len = s * rng.range(0.20, 0.28) * (1.1 - 0.4 * t);
        let base = Vec2::new(s * 0.5, s * (1.0 - t * 0.85));
        leaves.push((
            base + dir * len * 0.5,
            dir,
            len * 0.5,
            len * 0.2,
            rng.range(0.85, 1.15),
        ));
    }
    leaves.push((
        Vec2::new(s * 0.5, s * 0.07),
        Vec2::new(0.0, -1.0),
        s * 0.11,
        s * 0.05,
        1.0,
    ));
    Image::from_fn(size, size, |x, y| {
        let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
        let mut coverage: f32 = 0.0;
        let mut shade: f32 = 1.0;
        for &(c, dir, half_len, half_w, k) in &leaves {
            let d = p - c;
            let along = d.dot(dir) / half_len;
            let across = d.perp_dot(dir) / half_w;
            let r = along * along + across * across;
            // Signed distance in pixels, roughly.
            let dist = (r.sqrt() - 1.0) * half_w;
            let cov = (0.5 - dist).clamp(0.0, 1.0);
            if cov > coverage {
                coverage = cov;
                let vein = (1.0 - (across.abs() * 12.0).min(1.0)) * 0.25;
                shade = k * (0.8 + 0.25 * (1.0 - along.abs())) + vein;
            }
        }
        // The stem.
        let stem_x = (p.x - s * 0.5).abs();
        if p.y > s * 0.05 && stem_x < s * 0.008 + 0.5 {
            coverage = coverage.max(((s * 0.008 + 0.5) - stem_x).clamp(0.0, 1.0));
        }
        let c = tint * shade;
        [c.x, c.y, c.z, coverage]
    })
}

/// Bark: vertical furrows, tileable.
pub fn bark_texture(size: u32) -> Image {
    Image::from_fn(size, size, |x, y| {
        let n = tile_noise(x as f32 / 3.0, y as f32 / 24.0, (size / 3) as i32, 4) * 0.6
            + tile_noise(x as f32 / 9.0, y as f32 / 9.0, (size / 9) as i32, 8) * 0.4;
        let c = Vec3::new(0.16, 0.11, 0.07) * (0.6 + 0.8 * n);
        [c.x, c.y, c.z, 1.0]
    })
}

/// Shape of a test plant: an ellipsoid crown of foliage cards on a trunk.
#[derive(Clone, Copy, Debug)]
pub struct PlantShape {
    pub trunk_height: f32,
    pub trunk_radius: f32,
    pub crown_center: Vec3,
    pub crown_radii: Vec3,
    /// How many times the cards cover the crown's surface.
    pub coverage: f32,
    /// Cards at LOD 0 (LOD 1 and 2 use fewer, bigger ones).
    pub cards: u32,
}

impl PlantShape {
    pub fn tree() -> Self {
        Self {
            trunk_height: 2.6,
            trunk_radius: 0.12,
            crown_center: Vec3::new(0.0, 3.2, 0.0),
            crown_radii: Vec3::new(1.6, 1.4, 1.6),
            coverage: 4.0,
            cards: 218,
        }
    }

    /// A rounded shrub, 1 m: what the budget scene instances 5000 times.
    pub fn shrub() -> Self {
        Self {
            trunk_height: 0.3,
            trunk_radius: 0.04,
            crown_center: Vec3::new(0.0, 0.55, 0.0),
            crown_radii: Vec3::new(0.5, 0.45, 0.5),
            coverage: 4.0,
            cards: 218,
        }
    }

    /// A clipped hedge block, 1 m long, 0.8 m deep, 1.5 m high.
    pub fn hedge() -> Self {
        Self {
            trunk_height: 0.2,
            trunk_radius: 0.05,
            crown_center: Vec3::new(0.0, 0.8, 0.0),
            crown_radii: Vec3::new(0.62, 0.75, 0.45),
            coverage: 4.0,
            cards: 218,
        }
    }
}

/// A test plant at one level of detail: bark and foliage meshes sharing the
/// plant's origin (its base).
pub struct PlantLod {
    pub bark: MeshData,
    pub foliage: MeshData,
}

impl PlantLod {
    pub fn triangles(&self) -> usize {
        self.bark.triangles() + self.foliage.triangles()
    }
}

/// Three LODs of one plant from one seed: LOD 0 ~500 triangles (64 trunk +
/// 218 cards), LOD 1 ~160, LOD 2 ~48. Coarser levels use fewer cards on the
/// same crown, each larger so the silhouette and coverage stay alike.
pub fn plant_lods(shape: &PlantShape, seed: u32) -> [PlantLod; 3] {
    let levels = [(8, 4, shape.cards), (6, 2, 70), (4, 1, 20)];
    levels.map(|(segments, rings, cards)| {
        let mut bark = MeshData::default();
        bark.add_tube(
            Vec3::ZERO,
            shape.trunk_height + shape.crown_radii.y * 0.4,
            (shape.trunk_radius, shape.trunk_radius * 0.4),
            segments,
            rings,
            (0.0, 0.25),
        );
        let mut foliage = MeshData::default();
        let mut rng = Rng::new(seed);
        // Cards shingle the crown's surface (as visioncraft-flora does), so
        // a pixel sees only a few layers: together they cover the shell
        // about `coverage` times at every level.
        let r = shape.crown_radii;
        let shell = 4.0 * std::f32::consts::PI * ((r.x * r.y + r.y * r.z + r.x * r.z) / 3.0);
        let size = (shape.coverage * shell / cards as f32).sqrt();
        for _ in 0..cards {
            // Uniform direction, a little inside the shell.
            let dir = loop {
                let p = Vec3::new(
                    rng.range(-1.0, 1.0),
                    rng.range(-1.0, 1.0),
                    rng.range(-1.0, 1.0),
                );
                if p.length_squared() <= 1.0 && p.length_squared() > 0.01 {
                    break p.normalize();
                }
            };
            let base = shape.crown_center + dir * r * rng.range(0.7, 0.97);
            let outward = ((base - shape.crown_center) / (r * r)).normalize_or(Vec3::Y);
            // Mostly along the surface, leaning out and up, random twist.
            let tangent = outward.any_orthonormal_vector();
            let twist = Quat::from_axis_angle(outward, rng.range(0.0, std::f32::consts::TAU));
            let up = (twist * tangent * 0.8 + outward * 0.45 + Vec3::Y * 0.2).normalize();
            let side = up.cross(outward).normalize_or(Vec3::X);
            let half = side * size * 0.5;
            let tip = up * size;
            let base = base - tip * 0.4;
            let phase = rng.unit();
            let w0 = 0.6 + 0.15 * dir.y;
            let (wb, wt) = ([w0, phase], [1.0, phase]);
            let first = foliage.positions.len() as u32;
            for (p, uv, w) in [
                (base + tip - half, Vec2::new(0.0, 0.0), wt),
                (base + tip + half, Vec2::new(1.0, 0.0), wt),
                (base + half, Vec2::new(1.0, 1.0), wb),
                (base - half, Vec2::new(0.0, 1.0), wb),
            ] {
                // Normals bent fully outwards: cards shade as one crown.
                let n = ((p - shape.crown_center) / (shape.crown_radii * shape.crown_radii))
                    .normalize_or(Vec3::Y);
                foliage.vertex_full(p, n, uv, w);
            }
            foliage
                .indices
                .extend([0, 2, 1, 0, 3, 2].map(|i| first + i));
            foliage.next_face += 1;
        }
        PlantLod { bark, foliage }
    })
}

/// Materials for test plants.
pub struct PlantMaterials {
    pub bark: MaterialId,
    pub foliage: MaterialId,
}

pub fn plant_materials(r: &mut Renderer, tint: Vec3, seed: u32) -> PlantMaterials {
    let leaf = leaf_texture(512, tint, seed).upload(r, ColorSpace::Srgb, false);
    let bark_tex = bark_texture(128).upload(r, ColorSpace::Srgb, true);
    let bark = r.add_material(&Material {
        base_color_map: Some(bark_tex),
        roughness: 0.9,
        ..Default::default()
    });
    let foliage = r.add_material(&Material {
        base_color: [1.0, 1.0, 1.0, 1.0],
        base_color_map: Some(leaf),
        roughness: 0.6,
        alpha_mode: AlphaMode::Cutout { threshold: 0.5 },
        double_sided: true,
        foliage: true,
        ..Default::default()
    });
    PlantMaterials { bark, foliage }
}

/// Uploads a plant's three LODs as one LOD item (bark + foliage per level).
/// `distances`: max camera distance of LOD 0, 1 and 2.
pub fn add_plants(
    r: &mut Renderer,
    lods: &[PlantLod; 3],
    materials: &PlantMaterials,
    distances: [f32; 3],
    instances: &[Mat4],
) -> crate::ItemId {
    let mut parts = Vec::new();
    for (lod, d) in lods.iter().zip(distances) {
        parts.push((add(r, &lod.bark), materials.bark, d));
        parts.push((add(r, &lod.foliage), materials.foliage, d));
    }
    r.add_lod_item(&parts, instances)
}

/// A plant with mesh levels 0 and 1 up to `distances[0..2]` and an
/// impostor (baked from level 0, 12 views) up to `distances[2]`.
pub fn add_plants_with_impostor(
    r: &mut Renderer,
    lods: &[PlantLod; 3],
    materials: &PlantMaterials,
    distances: [f32; 3],
    instances: &[Mat4],
) -> (crate::ItemId, crate::ImpostorId) {
    let mut parts = Vec::new();
    for (lod, d) in lods.iter().zip(&distances[..2]) {
        parts.push((add(r, &lod.bark), materials.bark, *d));
        parts.push((add(r, &lod.foliage), materials.foliage, *d));
    }
    let impostor = r
        .bake_impostor(
            &[
                (parts[0].0, materials.bark),
                (parts[1].0, materials.foliage),
            ],
            None,
            12,
        )
        .expect("impostor");
    let item = r.add_lod_item(&parts, instances);
    r.set_item_impostor(item, impostor, distances[2])
        .expect("impostor level");
    (item, impostor)
}

/// A forest: `trees` trees and `understory` shrubs scattered over a square
/// of `half` metres around the origin, on a flat lawn with the infinite
/// ground, trees as meshes up to 6 / 12 m and impostors beyond, shrubs up to
/// 3 / 8 m (times the quality's LOD scale: a few dozen trees stay meshes). Returns the triangle count of a tree at LOD 0.
pub fn forest(r: &mut Renderer, trees: usize, understory: usize, half: f32) -> usize {
    let mut floor = MeshData::default();
    floor.add_box(
        Vec3::new(-half - 5.0, -0.3, -half - 5.0),
        Vec3::new(half + 5.0, 0.0, half + 5.0),
    );
    let floor = add(r, &floor);
    let lawn = lawn_texture(256).upload(r, ColorSpace::Srgb, true);
    let grass = r.add_material(&Material {
        base_color_map: Some(lawn),
        roughness: 0.95,
        projection: Projection::Triplanar { scale_m: 2.0 },
        variation: 0.6,
        ..Default::default()
    });
    r.add_item(floor, grass, &[Mat4::IDENTITY]);
    r.set_ground(Some(Ground {
        color: LAWN_ALBEDO,
        height: 0.0,
        ..Default::default()
    }));
    let mut rng = Rng::new(0x5eed);
    let mut scatter = |count: usize, scale: (f32, f32)| -> Vec<Mat4> {
        (0..count)
            .map(|_| {
                let (x, z) = (
                    (rng.unit() * 2.0 - 1.0) * half,
                    (rng.unit() * 2.0 - 1.0) * half,
                );
                let s = scale.0 + rng.unit() * (scale.1 - scale.0);
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(s),
                    Quat::from_rotation_y(rng.unit() * std::f32::consts::TAU),
                    Vec3::new(x, 0.0, z),
                )
            })
            .collect()
    };
    let tree_at = scatter(trees, (0.8, 1.6));
    let shrub_at = scatter(understory, (0.6, 1.4));
    let tree = plant_lods(&PlantShape::tree(), 42);
    let tree_mats = plant_materials(r, Vec3::new(0.16, 0.30, 0.07), 5);
    // In a forest this dense nothing is seen past ~150 m (and the haze
    // takes the rest): cull there, and the understory much sooner.
    add_plants_with_impostor(r, &tree, &tree_mats, [6.0, 12.0, 300.0], &tree_at);
    let shrub = plant_lods(&PlantShape::shrub(), 9);
    let shrub_mats = plant_materials(r, Vec3::new(0.14, 0.26, 0.06), 3);
    add_plants_with_impostor(r, &shrub, &shrub_mats, [3.0, 8.0, 80.0], &shrub_at);
    r.set_wind(&crate::Wind {
        direction: Vec3::new(1.0, 0.0, 0.4),
        strength: 0.6,
        gustiness: 0.5,
    });
    tree[0].triangles()
}

/// Inside the forest at eye level.
pub fn forest_inside_camera(aspect: f32) -> Camera {
    camera(
        Vec3::new(0.5, 1.7, 0.5),
        Vec3::new(20.0, 2.5, -14.0),
        aspect,
    )
}

/// Above the forest, looking over it.
pub fn forest_above_camera(aspect: f32) -> Camera {
    camera(
        Vec3::new(-60.0, 45.0, 70.0),
        Vec3::new(10.0, 0.0, -10.0),
        aspect,
    )
}

fn material(r: &mut Renderer, rgb: [f32; 3], metallic: f32, roughness: f32) -> MaterialId {
    r.add_material(&Material {
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        metallic,
        roughness,
        ..Default::default()
    })
}

fn add(r: &mut Renderer, mesh: &MeshData) -> MeshId {
    r.add_mesh(&mesh.mesh()).expect("demo meshes are valid")
}

fn ground(r: &mut Renderer, half: f32) {
    let mut m = MeshData::default();
    m.add_box(Vec3::new(-half, -0.3, -half), Vec3::new(half, 0.0, half));
    let mesh = add(r, &m);
    let mat = material(r, [0.42, 0.42, 0.40], 0.0, 0.9);
    r.add_item(mesh, mat, &[Mat4::IDENTITY]);
}

/// A camera: (view, projection, eye).
pub type Camera = (Mat4, Mat4, Vec3);

pub fn camera(eye: Vec3, target: Vec3, aspect: f32) -> Camera {
    (
        Mat4::look_at_rh(eye, target, Vec3::Y),
        Mat4::perspective_rh(50f32.to_radians(), aspect, 0.1, 1000.0),
        eye,
    )
}

/// (a) Spheres on a plane: front row dielectric, back row metal, roughness
/// rising left to right.
pub fn spheres(r: &mut Renderer) {
    ground(r, 30.0);
    let mut ball = MeshData::default();
    ball.add_sphere(Vec3::ZERO, 0.4, 48, 24);
    let ball = add(r, &ball);
    for row in 0..2 {
        for col in 0..5 {
            let rough = 0.05 + 0.9 * col as f32 / 4.0;
            let metal = row as f32;
            let color = if row == 0 {
                [0.7, 0.12, 0.08]
            } else {
                [0.95, 0.75, 0.4]
            };
            let mat = material(r, color, metal, rough);
            let at = Vec3::new(
                col as f32 * 1.0 - 2.0 + row as f32 * 0.5,
                0.4,
                -row as f32 * 1.4,
            );
            r.add_item(ball, mat, &[Mat4::from_translation(at)]);
        }
    }
}

pub fn spheres_camera(aspect: f32) -> Camera {
    camera(Vec3::new(0.0, 2.4, 5.4), Vec3::new(0.25, 0.3, -0.8), aspect)
}

pub fn noon_sun() -> Sun {
    Sun::from_angles(160.0, 60.0)
}

/// Mesh ids of the house scene, for highlight tests.
pub struct House {
    pub block: MeshId,
    /// Face id of the door at the back of the recessed doorway.
    pub door_face: u32,
}

/// (b) A ground slab, a house block (8 x 6 x 3 m) with a recessed doorway in
/// its south (+Z) wall and an overhanging flat roof, three thin-legged tables
/// in front of it and a row of instanced boxes as a hedge along the west side.
pub fn house(r: &mut Renderer) -> House {
    ground(r, 20.0);
    let mut block = MeshData::default();
    block
        .add_box(Vec3::new(-4.0, 0.0, -3.0), Vec3::new(-0.5, 3.0, 3.0))
        .add_box(Vec3::new(0.5, 0.0, -3.0), Vec3::new(4.0, 3.0, 3.0))
        .add_box(Vec3::new(-0.5, 2.2, -3.0), Vec3::new(0.5, 3.0, 3.0));
    let door_face = block.next_face() + 4; // +Z face of the next box
    block.add_box(Vec3::new(-0.5, 0.0, -3.0), Vec3::new(0.5, 2.2, 2.6));
    let block_id = add(r, &block);
    let walls = material(r, [0.80, 0.78, 0.74], 0.0, 0.8);
    r.add_item(block_id, walls, &[Mat4::IDENTITY]);

    let mut roof = MeshData::default();
    roof.add_box(Vec3::new(-4.8, 3.0, -3.8), Vec3::new(4.8, 3.25, 3.8));
    let roof = add(r, &roof);
    let roof_mat = material(r, [0.32, 0.33, 0.35], 0.0, 0.6);
    r.add_item(roof, roof_mat, &[Mat4::IDENTITY]);

    let mut table = MeshData::default();
    table.add_box(Vec3::new(-0.6, 0.72, -0.4), Vec3::new(0.6, 0.76, 0.4));
    for (x, z) in [(-0.54, -0.34), (0.54, -0.34), (0.54, 0.34), (-0.54, 0.34)] {
        table.add_box(
            Vec3::new(x - 0.02, 0.0, z - 0.02),
            Vec3::new(x + 0.02, 0.72, z + 0.02),
        );
    }
    let table = add(r, &table);
    let wood = material(r, [0.50, 0.33, 0.18], 0.0, 0.55);
    let tables = [
        Mat4::from_translation(Vec3::new(-2.4, 0.0, 5.6)),
        Mat4::from_rotation_translation(Quat::from_rotation_y(0.35), Vec3::new(0.6, 0.0, 6.4)),
        Mat4::from_translation(Vec3::new(3.0, 0.0, 5.2)),
    ];
    r.add_item(table, wood, &tables);

    let mut unit = MeshData::default();
    unit.add_box(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5));
    let unit = add(r, &unit);
    let leaves = material(r, [0.10, 0.24, 0.07], 0.0, 0.9);
    let hedge: Vec<Mat4> = (0..12)
        .map(|i| {
            Mat4::from_scale_rotation_translation(
                Vec3::new(0.7, 1.0 + 0.1 * (i % 3) as f32, 1.1),
                Quat::IDENTITY,
                Vec3::new(-8.0, 0.0, -4.4 + i as f32 * 1.2),
            )
        })
        .collect();
    r.add_item(unit, leaves, &hedge);
    House {
        block: block_id,
        door_face,
    }
}

pub fn house_camera(aspect: f32) -> Camera {
    camera(Vec3::new(2.0, 2.4, 16.0), Vec3::new(-0.5, 1.0, 3.0), aspect)
}

/// Low evening sun from the west-south-west.
pub fn evening_sun() -> Sun {
    Sun::from_angles(240.0, 12.0)
}

/// Afternoon sun from the south-west.
pub fn afternoon_sun() -> Sun {
    Sun::from_angles(225.0, 38.0)
}

/// Late dusk: the sun 8 degrees below the horizon (nautical twilight).
pub fn dusk_sun() -> Sun {
    Sun::from_angles(300.0, -4.0)
}

/// Low sun, warm light, long shadows.
pub fn golden_sun() -> Sun {
    Sun::from_angles(255.0, 6.0)
}

/// An overcast sky as an equirectangular HDRI (`width` x `width / 2`,
/// linear RGB): the CIE standard overcast luminance distribution,
/// L(theta) = Lz (1 + 2 cos theta) / 3 (Moon & Spencer 1942), with Lz = 1,
/// slightly blue-grey, over a dark grey-green ground. Use with
/// `Sky::Hdri { intensity: ~6000 }` (a bright overcast day, ~15 klx) and a
/// sun of zero illuminance.
pub fn overcast_hdri(width: u32) -> Vec<f32> {
    let height = width / 2;
    let mut out = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        let cos_theta = (std::f32::consts::PI * (y as f32 + 0.5) / height as f32).cos();
        for x in 0..width {
            // Faint large-scale cloud structure so the sky is not a gradient.
            let n = tile_noise(x as f32 / 24.0, y as f32 / 12.0, (width / 24) as i32, 7);
            let c = if cos_theta >= 0.0 {
                Vec3::new(0.94, 0.97, 1.0) * (1.0 + 2.0 * cos_theta) / 3.0 * (0.9 + 0.2 * n)
            } else {
                Vec3::new(0.05, 0.06, 0.045)
            };
            out.extend(c.to_array());
        }
    }
    out
}

/// Night: the sun far below the horizon (only the night sky floor).
pub fn night_sun() -> Sun {
    Sun::from_angles(0.0, -30.0)
}

/// (e) Textured walls without UVs: an L of brick walls and a pillar,
/// triplanar-projected brick with a normal map; the pillar weathered.
pub fn brick_walls(r: &mut Renderer) {
    ground(r, 20.0);
    let (albedo, normal, orm) = brick_textures(512);
    let albedo = albedo.upload(r, ColorSpace::Srgb, true);
    let normal = normal.upload(r, ColorSpace::Linear, true);
    let orm = orm.upload(r, ColorSpace::Linear, true);
    let brick = Material {
        base_color_map: Some(albedo),
        normal_map: Some(normal),
        orm_map: Some(orm),
        roughness: 1.0,
        projection: Projection::Triplanar { scale_m: 1.0 },
        ..Default::default()
    };
    let plain = r.add_material(&brick);
    let weathered = r.add_material(&Material {
        weathering: 0.8,
        ..brick
    });
    let mut walls = MeshData::default();
    walls
        .add_box(Vec3::new(-3.0, 0.0, -0.15), Vec3::new(3.0, 2.6, 0.15))
        .add_box(Vec3::new(-3.0, 0.0, 0.15), Vec3::new(-2.7, 2.6, 4.0));
    let walls = add(r, &walls);
    r.add_item(walls, plain, &[Mat4::IDENTITY]);
    let mut pillar = MeshData::default();
    pillar.add_box(Vec3::new(-0.35, 0.0, -0.35), Vec3::new(0.35, 1.8, 0.35));
    pillar.add_sphere(Vec3::new(0.0, 2.15, 0.0), 0.35, 32, 16);
    let pillar = add(r, &pillar);
    r.add_item(
        pillar,
        weathered,
        &[Mat4::from_translation(Vec3::new(1.2, 0.0, 2.2))],
    );
}

pub fn brick_walls_camera(aspect: f32) -> Camera {
    camera(Vec3::new(3.6, 1.6, 6.0), Vec3::new(-0.4, 1.2, 0.6), aspect)
}

/// (f) One foliage-card tree (LOD 0) on a plain ground, wind blowing.
pub fn foliage_tree(r: &mut Renderer) {
    ground(r, 20.0);
    let lods = plant_lods(&PlantShape::tree(), 42);
    let mats = plant_materials(r, Vec3::new(0.16, 0.30, 0.07), 5);
    add_plants(r, &lods, &mats, [f32::INFINITY; 3], &[Mat4::IDENTITY]);
    // A white wall behind it to catch the cutout shadow.
    let mut wall = MeshData::default();
    wall.add_box(Vec3::new(-4.0, 0.0, -3.0), Vec3::new(4.0, 3.5, -2.8));
    let wall = add(r, &wall);
    let white = material(r, [0.8, 0.8, 0.78], 0.0, 0.9);
    r.add_item(wall, white, &[Mat4::IDENTITY]);
    r.set_wind(&crate::Wind {
        direction: Vec3::new(1.0, 0.0, 0.3),
        strength: 1.5,
        gustiness: 0.5,
    });
}

pub fn foliage_camera(aspect: f32) -> Camera {
    camera(Vec3::new(1.5, 2.2, 7.5), Vec3::new(0.0, 2.4, 0.0), aspect)
}

/// The sun for the foliage scene: behind-left and high, so the wall catches
/// the tree's shadow.
pub fn foliage_sun() -> Sun {
    Sun::from_angles(160.0, 55.0)
}

/// (g) The same tree at 6, 18 and 40 m with LOD switch distances 12 / 30 /
/// infinity: each shows a different level.
pub fn lod_row(r: &mut Renderer) {
    ground(r, 80.0);
    let lods = plant_lods(&PlantShape::tree(), 42);
    let mats = plant_materials(r, Vec3::new(0.16, 0.30, 0.07), 5);
    let at = [
        Vec3::new(-1.5, 0.0, -6.0),
        Vec3::new(3.0, 0.0, -18.0),
        Vec3::new(10.0, 0.0, -40.0),
    ];
    add_plants(
        r,
        &lods,
        &mats,
        [12.0, 30.0, f32::INFINITY],
        &at.map(Mat4::from_translation),
    );
}

pub fn lod_camera(aspect: f32) -> Camera {
    camera(Vec3::new(0.0, 1.7, 0.0), Vec3::new(2.5, 2.5, -20.0), aspect)
}

/// (h) A window: a glass pane in front of a dark room (a box open only
/// through the window), with a lamp-less interior and furniture.
pub fn glass_room(r: &mut Renderer) {
    ground(r, 20.0);
    let mut room = MeshData::default();
    // Front wall with a 2 x 1.4 m opening, back, sides, roof, floor.
    room.add_box(Vec3::new(-3.0, 0.0, 0.0), Vec3::new(-1.0, 3.0, 0.2))
        .add_box(Vec3::new(1.0, 0.0, 0.0), Vec3::new(3.0, 3.0, 0.2))
        .add_box(Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.8, 0.2))
        .add_box(Vec3::new(-1.0, 2.2, 0.0), Vec3::new(1.0, 3.0, 0.2))
        .add_box(Vec3::new(-3.0, 0.0, -4.0), Vec3::new(3.0, 3.0, -3.8))
        .add_box(Vec3::new(-3.0, 0.0, -3.8), Vec3::new(-2.8, 3.0, 0.0))
        .add_box(Vec3::new(2.8, 0.0, -3.8), Vec3::new(3.0, 3.0, 0.0))
        .add_box(Vec3::new(-3.0, 3.0, -4.0), Vec3::new(3.0, 3.2, 0.2));
    let room = add(r, &room);
    let wall = material(r, [0.75, 0.73, 0.70], 0.0, 0.85);
    r.add_item(room, wall, &[Mat4::IDENTITY]);
    let mut furniture = MeshData::default();
    furniture
        .add_box(Vec3::new(-1.5, 0.0, -3.0), Vec3::new(0.2, 0.9, -2.2))
        .add_box(Vec3::new(0.8, 0.0, -2.0), Vec3::new(1.6, 1.8, -1.5));
    let furniture = add(r, &furniture);
    let red = material(r, [0.6, 0.15, 0.1], 0.0, 0.6);
    r.add_item(furniture, red, &[Mat4::IDENTITY]);
    let mut pane = MeshData::default();
    pane.add_quad(
        [
            Vec3::new(-1.0, 2.2, 0.1),
            Vec3::new(-1.0, 0.8, 0.1),
            Vec3::new(1.0, 0.8, 0.1),
            Vec3::new(1.0, 2.2, 0.1),
        ],
        [[0.0; 2]; 4],
    );
    let pane = add(r, &pane);
    let glass = r.add_material(&Material {
        base_color: [0.85, 0.92, 0.9, 0.06],
        roughness: 0.02,
        double_sided: true,
        surface: Surface::Glass,
        ..Default::default()
    });
    r.add_item(pane, glass, &[Mat4::IDENTITY]);
}

pub fn glass_camera(aspect: f32) -> Camera {
    camera(Vec3::new(1.8, 1.7, 5.0), Vec3::new(0.0, 1.4, 0.0), aspect)
}

fn pond_height(x: f32, z: f32) -> f32 {
    // An oval basin 0.9 m deep, sloping up to the lawn at y = 0.
    let d = ((x / 4.0).powi(2) + (z / 2.6).powi(2)).sqrt();
    if d >= 1.0 {
        0.0
    } else {
        -0.9 * (1.0 - d * d).powf(0.8) + 0.08
    }
}

/// (i) A pond: a sloped basin in the ground, water just below the rim, a
/// few stones, a planting of shrubs behind.
pub fn pond(r: &mut Renderer) {
    let mut terrain = MeshData::default();
    terrain.add_terrain(12.0, 96, pond_height);
    let terrain = add(r, &terrain);
    let lawn = lawn_texture(256).upload(r, ColorSpace::Srgb, true);
    let soil = r.add_material(&Material {
        base_color: [1.0, 1.0, 1.0, 1.0],
        base_color_map: Some(lawn),
        roughness: 0.95,
        projection: Projection::Triplanar { scale_m: 2.0 },
        ..Default::default()
    });
    r.add_item(terrain, soil, &[Mat4::IDENTITY]);
    let mut stone = MeshData::default();
    stone.add_sphere(Vec3::ZERO, 0.35, 16, 8);
    let stone = add(r, &stone);
    let grey = material(r, [0.22, 0.21, 0.19], 0.0, 0.8);
    let stones: Vec<Mat4> = [
        (3.7, 0.5),
        (3.2, -1.3),
        (-3.6, 1.0),
        (1.0, 2.5),
        (-1.5, -2.45),
    ]
    .iter()
    .enumerate()
    .map(|(i, &(x, z))| {
        Mat4::from_scale_rotation_translation(
            Vec3::new(1.0 + 0.3 * i as f32 % 0.7, 0.5, 0.8),
            Quat::from_rotation_y(i as f32),
            Vec3::new(x, pond_height(x, z) + 0.05, z),
        )
    })
    .collect();
    r.add_item(stone, grey, &stones);
    let mut water = MeshData::default();
    water.add_quad(
        [
            Vec3::new(-4.2, -0.04, -2.8),
            Vec3::new(-4.2, -0.04, 2.8),
            Vec3::new(4.2, -0.04, 2.8),
            Vec3::new(4.2, -0.04, -2.8),
        ],
        [[0.0; 2]; 4],
    );
    let water = add(r, &water);
    let pond_water = r.add_material(&Material {
        base_color: [0.02, 0.04, 0.03, 0.45],
        roughness: 0.05,
        surface: Surface::Water,
        ..Default::default()
    });
    r.add_item(water, pond_water, &[Mat4::IDENTITY]);
    let lods = plant_lods(&PlantShape::shrub(), 9);
    let mats = plant_materials(r, Vec3::new(0.14, 0.26, 0.06), 3);
    let shrubs: Vec<Mat4> = (0..9)
        .map(|i| {
            let x = -4.0 + i as f32;
            Mat4::from_scale_rotation_translation(
                Vec3::splat(1.0 + 0.2 * (i % 3) as f32),
                Quat::from_rotation_y(i as f32 * 2.1),
                Vec3::new(x, 0.0, -4.0 - 0.4 * (i % 2) as f32),
            )
        })
        .collect();
    add_plants(r, &lods, &mats, [15.0, 40.0, f32::INFINITY], &shrubs);
}

pub fn pond_camera(aspect: f32) -> Camera {
    camera(Vec3::new(2.0, 1.6, 7.5), Vec3::new(0.0, -0.3, 0.0), aspect)
}

/// Warm garden-lamp colour (about 2700 K, luminance 1).
pub const WARM: Vec3 = Vec3::new(1.25, 0.95, 0.62);

/// Three point lights (wall lamps, a lamp by the tables) and a spot on the
/// hedge, for the house scene at night.
pub fn house_lights() -> Vec<Light> {
    let point = |p: Vec3, lm: f32| Light {
        position: p,
        kind: LightKind::Point,
        color: WARM,
        intensity: lm,
        range_m: 9.0,
    };
    vec![
        point(Vec3::new(-2.0, 2.4, 3.3), 600.0),
        point(Vec3::new(2.2, 2.4, 3.3), 600.0),
        point(Vec3::new(0.5, 1.0, 5.6), 400.0),
        Light {
            position: Vec3::new(-5.5, 3.5, 2.0),
            kind: LightKind::Spot {
                direction: Vec3::new(-0.55, -0.8, -0.1).normalize(),
                inner: 0.35,
                outer: 0.6,
            },
            color: Vec3::new(0.85, 0.95, 1.2),
            intensity: 1500.0,
            range_m: 14.0,
        },
    ]
}

/// Switches for the budget scene, to measure what each feature costs.
#[derive(Clone, Copy, Debug)]
pub struct BudgetOptions {
    /// Brick maps (triplanar, normal, ORM) on the static geometry.
    pub textured_static: bool,
    /// Alpha-cut foliage cards (otherwise opaque cards).
    pub cutout_foliage: bool,
    /// LOD switching at 12 / 30 m (otherwise LOD 0 everywhere).
    pub lods: bool,
    pub wind: bool,
    /// The 5000 shrubs (off: only the static geometry).
    pub plants: bool,
}

impl Default for BudgetOptions {
    fn default() -> Self {
        Self {
            textured_static: true,
            cutout_foliage: true,
            lods: true,
            wind: true,
            plants: true,
        }
    }
}

/// The bench's budget scene: ~200k triangles of static geometry (20 meshes
/// of boxes, like a CAD model's parts, brick-textured with triplanar
/// projection) plus 5000 alpha-cut, textured, wind-animated shrubs (500
/// triangles at LOD 0, LODs switching at 12 / 30 m). Returns (static
/// triangles, instanced triangles at LOD 0).
pub fn budget(r: &mut Renderer) -> (usize, usize) {
    budget_with(r, &BudgetOptions::default())
}

pub fn budget_with(r: &mut Renderer, o: &BudgetOptions) -> (usize, usize) {
    ground(r, 40.0);
    let mut rng = Rng::new(0x9e37_79b9);
    let mut static_tris = 0;
    let (albedo, normal, orm) = brick_textures(512);
    let albedo = albedo.upload(r, ColorSpace::Srgb, true);
    let normal = normal.upload(r, ColorSpace::Linear, true);
    let orm = orm.upload(r, ColorSpace::Linear, true);
    let colors = [
        [1.0, 1.0, 1.0],
        [0.7, 0.7, 0.75],
        [0.9, 0.8, 0.7],
        [0.8, 0.85, 0.9],
    ];
    for part in 0..20 {
        let mut m = MeshData::default();
        // A "building" of 834 boxes (10k triangles) in a 6 m cell.
        let origin = Vec3::new(
            (part % 5) as f32 * 10.0 - 20.0,
            0.0,
            (part / 5) as f32 * 10.0 - 20.0,
        );
        for _ in 0..834 {
            let p = origin + Vec3::new(rng.unit() * 6.0, rng.unit() * 4.0, rng.unit() * 6.0);
            let s = Vec3::new(
                0.1 + rng.unit() * 0.6,
                0.05 + rng.unit() * 0.5,
                0.1 + rng.unit() * 0.6,
            );
            m.add_box(p, p + s);
        }
        static_tris += m.triangles();
        let mesh = add(r, &m);
        let c = colors[part % 4];
        let textured = Material {
            base_color: [c[0], c[1], c[2], 1.0],
            base_color_map: Some(albedo),
            normal_map: Some(normal),
            orm_map: Some(orm),
            roughness: 1.0,
            projection: Projection::Triplanar { scale_m: 1.0 },
            ..Default::default()
        };
        let mat = if o.textured_static {
            r.add_material(&textured)
        } else {
            material(r, [c[0] * 0.6, c[1] * 0.5, c[2] * 0.45], 0.0, 0.8)
        };
        r.add_item(mesh, mat, &[Mat4::IDENTITY]);
    }
    let lods = plant_lods(&PlantShape::shrub(), 77);
    let per = lods[0].triangles();
    if !o.plants {
        return (static_tris, 0);
    }
    let mats = plant_materials(r, Vec3::new(0.15, 0.3, 0.07), 2);
    if !o.cutout_foliage {
        r.set_material(
            mats.foliage,
            &Material {
                base_color: [0.12, 0.3, 0.08, 1.0],
                roughness: 0.6,
                double_sided: true,
                foliage: true,
                ..Default::default()
            },
        );
    }
    let shrubs: Vec<Mat4> = (0..5000)
        .map(|i| {
            let (x, z) = ((i % 71) as f32 * 0.7 - 24.5, (i / 71) as f32 * 0.7 - 24.5);
            let s = 0.3 + rng.unit() * 0.4; // as the previous (box) budget scene
            Mat4::from_scale_rotation_translation(
                Vec3::new(s, s * (0.8 + rng.unit() * 0.6), s),
                Quat::from_rotation_y(rng.unit() * std::f32::consts::TAU),
                Vec3::new(x + rng.unit() * 0.3, 0.0, z + rng.unit() * 0.3),
            )
        })
        .collect();
    let distances = if o.lods {
        [12.0, 30.0, f32::INFINITY]
    } else {
        [f32::INFINITY, f32::INFINITY, f32::INFINITY]
    };
    if o.lods {
        add_plants(r, &lods, &mats, distances, &shrubs);
    } else {
        let parts = [
            (add(r, &lods[0].bark), mats.bark, f32::INFINITY),
            (add(r, &lods[0].foliage), mats.foliage, f32::INFINITY),
        ];
        r.add_lod_item(&parts, &shrubs);
    }
    r.set_wind(&crate::Wind {
        direction: Vec3::new(1.0, 0.0, 0.4),
        strength: if o.wind { 1.0 } else { 0.0 },
        gustiness: 0.5,
    });
    (static_tris, per * shrubs.len())
}

/// (j) The garden of the beauty shots: a house block with brick walls, two
/// windows and a flat roof, a paved terrace, a lawn, a pond, a hedge row
/// and a few trees. Returns the garden lights (switch on for dusk).
pub fn garden(r: &mut Renderer) -> Vec<Light> {
    // Lawn with the pond basin, offset to the south-east of the house.
    let pond_at = Vec3::new(5.5, 0.0, 8.0);
    let mut terrain = MeshData::default();
    terrain.add_terrain(30.0, 160, |x, z| pond_height(x - pond_at.x, z - pond_at.z));
    let terrain = add(r, &terrain);
    let lawn = lawn_texture(256).upload(r, ColorSpace::Srgb, true);
    let grass = r.add_material(&Material {
        base_color_map: Some(lawn),
        base_color: [1.0; 4],
        roughness: 0.95,
        projection: Projection::Triplanar { scale_m: 2.0 },
        variation: 0.6,
        ..Default::default()
    });
    r.add_item(terrain, grass, &[Mat4::IDENTITY]);
    // The world goes on past the 60 m lawn: an endless field of the same
    // colour, into the haze.
    r.set_ground(Some(Ground {
        color: LAWN_ALBEDO,
        ..Default::default()
    }));

    // House: 9 x 7 x 3.2 m with two window openings in the south wall.
    let (albedo, normal, orm) = brick_textures(512);
    let albedo = albedo.upload(r, ColorSpace::Srgb, true);
    let normal = normal.upload(r, ColorSpace::Linear, true);
    let orm = orm.upload(r, ColorSpace::Linear, true);
    let brick = r.add_material(&Material {
        base_color_map: Some(albedo),
        normal_map: Some(normal),
        orm_map: Some(orm),
        roughness: 1.0,
        weathering: 0.35,
        projection: Projection::Triplanar { scale_m: 1.0 },
        ..Default::default()
    });
    let (x0, x1, z0, z1, h) = (-4.5, 4.5, -3.5, 3.5, 3.2);
    let mut walls = MeshData::default();
    // South wall (z1) with windows at x -3..-1 and 1..3, sill 0.9, head 2.3.
    walls
        .add_box(Vec3::new(x0, 0.0, z1 - 0.3), Vec3::new(-3.0, h, z1))
        .add_box(Vec3::new(-1.0, 0.0, z1 - 0.3), Vec3::new(1.0, h, z1))
        .add_box(Vec3::new(3.0, 0.0, z1 - 0.3), Vec3::new(x1, h, z1))
        .add_box(Vec3::new(-3.0, 0.0, z1 - 0.3), Vec3::new(-1.0, 0.9, z1))
        .add_box(Vec3::new(1.0, 0.0, z1 - 0.3), Vec3::new(3.0, 0.9, z1))
        .add_box(Vec3::new(-3.0, 2.3, z1 - 0.3), Vec3::new(-1.0, h, z1))
        .add_box(Vec3::new(1.0, 2.3, z1 - 0.3), Vec3::new(3.0, h, z1))
        .add_box(Vec3::new(x0, 0.0, z0), Vec3::new(x1, h, z0 + 0.3))
        .add_box(
            Vec3::new(x0, 0.0, z0 + 0.3),
            Vec3::new(x0 + 0.3, h, z1 - 0.3),
        )
        .add_box(
            Vec3::new(x1 - 0.3, 0.0, z0 + 0.3),
            Vec3::new(x1, h, z1 - 0.3),
        );
    let walls = add(r, &walls);
    r.add_item(walls, brick, &[Mat4::IDENTITY]);
    let mut inside = MeshData::default();
    inside
        .add_box(Vec3::new(x0, 0.0, z0), Vec3::new(x1, 0.05, z1))
        .add_box(Vec3::new(-2.5, 0.05, -2.5), Vec3::new(0.5, 0.8, -1.6))
        .add_box(Vec3::new(2.0, 0.05, -3.0), Vec3::new(3.6, 2.0, -2.6));
    let inside = add(r, &inside);
    let interior = material(r, [0.55, 0.5, 0.45], 0.0, 0.8);
    r.add_item(inside, interior, &[Mat4::IDENTITY]);
    let mut roof = MeshData::default();
    roof.add_box(
        Vec3::new(x0 - 0.5, h, z0 - 0.5),
        Vec3::new(x1 + 0.5, h + 0.3, z1 + 0.5),
    );
    let roof = add(r, &roof);
    let roof_mat = material(r, [0.2, 0.2, 0.22], 0.0, 0.7);
    r.add_item(roof, roof_mat, &[Mat4::IDENTITY]);
    let mut frames = MeshData::default();
    for x in [-3.0, 1.0] {
        frames
            .add_box(
                Vec3::new(x, 0.88, z1 - 0.2),
                Vec3::new(x + 2.0, 0.95, z1 + 0.06),
            )
            .add_box(
                Vec3::new(x + 0.97, 0.95, z1 - 0.18),
                Vec3::new(x + 1.03, 2.3, z1 - 0.12),
            );
    }
    let frames = add(r, &frames);
    let alu = material(r, [0.6, 0.6, 0.62], 1.0, 0.35);
    r.add_item(frames, alu, &[Mat4::IDENTITY]);
    let mut pane = MeshData::default();
    pane.add_quad(
        [
            Vec3::new(0.0, 2.3, 0.0),
            Vec3::new(0.0, 0.95, 0.0),
            Vec3::new(2.0, 0.95, 0.0),
            Vec3::new(2.0, 2.3, 0.0),
        ],
        [[0.0; 2]; 4],
    );
    let pane = add(r, &pane);
    let glass = r.add_material(&Material {
        base_color: [0.85, 0.92, 0.9, 0.05],
        roughness: 0.02,
        double_sided: true,
        surface: Surface::Glass,
        ..Default::default()
    });
    r.add_item(
        pane,
        glass,
        &[-3.0, 1.0].map(|x| Mat4::from_translation(Vec3::new(x, 0.0, z1 - 0.15))),
    );

    // Paved terrace along the south wall.
    let (pave, pave_n) = paving_textures(512);
    let pave = pave.upload(r, ColorSpace::Srgb, true);
    let pave_n = pave_n.upload(r, ColorSpace::Linear, true);
    let paving = r.add_material(&Material {
        base_color_map: Some(pave),
        normal_map: Some(pave_n),
        roughness: 0.75,
        weathering: 0.3,
        projection: Projection::Triplanar { scale_m: 1.0 },
        variation: 0.25,
        ..Default::default()
    });
    let mut terrace = MeshData::default();
    terrace.add_box(Vec3::new(-5.0, -0.1, z1), Vec3::new(3.0, 0.04, z1 + 4.0));
    let terrace = add(r, &terrace);
    r.add_item(terrace, paving, &[Mat4::IDENTITY]);

    // Garden table on the terrace.
    let mut table = MeshData::default();
    table.add_box(Vec3::new(-0.7, 0.72, -0.45), Vec3::new(0.7, 0.76, 0.45));
    for (x, z) in [(-0.64, -0.39), (0.64, -0.39), (0.64, 0.39), (-0.64, 0.39)] {
        table.add_box(
            Vec3::new(x - 0.025, 0.0, z - 0.025),
            Vec3::new(x + 0.025, 0.72, z + 0.025),
        );
    }
    let table = add(r, &table);
    let wood = material(r, [0.26, 0.16, 0.09], 0.0, 0.55);
    r.add_item(
        table,
        wood,
        &[Mat4::from_translation(Vec3::new(-1.5, 0.04, z1 + 2.0))],
    );

    // The pond.
    let mut water = MeshData::default();
    water.add_quad(
        [
            Vec3::new(-4.2, -0.04, -2.8),
            Vec3::new(-4.2, -0.04, 2.8),
            Vec3::new(4.2, -0.04, 2.8),
            Vec3::new(4.2, -0.04, -2.8),
        ],
        [[0.0; 2]; 4],
    );
    let water = add(r, &water);
    let pond_water = r.add_material(&Material {
        base_color: [0.02, 0.04, 0.03, 0.45],
        roughness: 0.05,
        surface: Surface::Water,
        ..Default::default()
    });
    r.add_item(water, pond_water, &[Mat4::from_translation(pond_at)]);
    let mut stone = MeshData::default();
    stone.add_sphere(Vec3::ZERO, 0.35, 16, 8);
    let stone = add(r, &stone);
    let grey = material(r, [0.22, 0.21, 0.19], 0.0, 0.8);
    let stones: Vec<Mat4> = [
        (3.9, 0.3),
        (3.3, -1.4),
        (-3.8, 0.9),
        (0.8, 2.6),
        (-1.6, -2.5),
    ]
    .iter()
    .enumerate()
    .map(|(i, &(x, z))| {
        Mat4::from_scale_rotation_translation(
            Vec3::new(1.0 + 0.15 * i as f32, 0.5, 0.8),
            Quat::from_rotation_y(i as f32),
            pond_at + Vec3::new(x, 0.03, z),
        )
    })
    .collect();
    r.add_item(stone, grey, &stones);

    // Hedge row along the west edge, trees to the east and behind.
    let hedge_lods = plant_lods(&PlantShape::hedge(), 13);
    let hedge_mats = plant_materials(r, Vec3::new(0.10, 0.22, 0.06), 17);
    let hedge: Vec<Mat4> = (0..18)
        .map(|i| {
            Mat4::from_scale_rotation_translation(
                Vec3::new(1.1, 1.0 + 0.05 * (i % 3) as f32, 1.0),
                Quat::from_rotation_y(
                    std::f32::consts::FRAC_PI_2 + (i % 2) as f32 * std::f32::consts::PI,
                ),
                Vec3::new(-9.0, 0.0, -6.0 + i as f32 * 1.0),
            )
        })
        .collect();
    add_plants(
        r,
        &hedge_lods,
        &hedge_mats,
        [15.0, 35.0, f32::INFINITY],
        &hedge,
    );
    let tree_lods = plant_lods(&PlantShape::tree(), 42);
    let tree_mats = plant_materials(r, Vec3::new(0.16, 0.30, 0.07), 5);
    let trees: Vec<Mat4> = [
        (11.0, 2.0, 1.3),
        (9.5, -6.0, 1.1),
        (-6.5, -8.0, 1.5),
        (13.0, 13.0, 1.0),
    ]
    .iter()
    .enumerate()
    .map(|(i, &(x, z, s))| {
        Mat4::from_scale_rotation_translation(
            Vec3::splat(s),
            Quat::from_rotation_y(i as f32 * 1.7),
            Vec3::new(x, 0.0, z),
        )
    })
    .collect();
    add_plants(
        r,
        &tree_lods,
        &tree_mats,
        [20.0, 45.0, f32::INFINITY],
        &trees,
    );
    let shrub_lods = plant_lods(&PlantShape::shrub(), 9);
    let shrub_mats = plant_materials(r, Vec3::new(0.14, 0.26, 0.06), 3);
    let shrubs: Vec<Mat4> = [
        (9.5, 5.0),
        (10.3, 6.2),
        (1.0, 11.5),
        (2.3, 12.0),
        (-7.5, 9.0),
        (4.4, 4.6),
    ]
    .iter()
    .enumerate()
    .map(|(i, &(x, z))| {
        Mat4::from_scale_rotation_translation(
            Vec3::splat(1.0 + 0.25 * (i % 3) as f32),
            Quat::from_rotation_y(i as f32 * 2.3),
            Vec3::new(x, 0.0, z),
        )
    })
    .collect();
    add_plants(
        r,
        &shrub_lods,
        &shrub_mats,
        [15.0, 40.0, f32::INFINITY],
        &shrubs,
    );

    // Garden lamps: two wall lights, a post lamp by the pond, a spot
    // up-lighting the big tree. Emissive bulbs make the lamps visible.
    let mut bulb = MeshData::default();
    bulb.add_sphere(Vec3::ZERO, 0.07, 12, 6);
    let bulb = add(r, &bulb);
    let glow = r.add_material(&Material {
        base_color: [1.0; 4],
        emissive: WARM.to_array(),
        emissive_strength: 4000.0,
        ..Default::default()
    });
    let lamps = [
        Vec3::new(-3.7, 2.4, z1 + 0.12),
        Vec3::new(3.7, 2.4, z1 + 0.12),
        pond_at + Vec3::new(-4.6, 1.0, -2.0),
        Vec3::new(-1.5, 0.9, z1 + 2.0),
    ];
    r.add_item(bulb, glow, &lamps.map(Mat4::from_translation));
    let mut post = MeshData::default();
    post.add_box(Vec3::new(-0.04, 0.0, -0.04), Vec3::new(0.04, 0.93, 0.04));
    let post = add(r, &post);
    let dark = material(r, [0.05, 0.05, 0.05], 0.8, 0.4);
    r.add_item(
        post,
        dark,
        &[Mat4::from_translation(lamps[2] - Vec3::Y * 1.0)],
    );
    let point = |p: Vec3, lm: f32, range: f32| Light {
        position: p,
        kind: LightKind::Point,
        color: WARM,
        intensity: lm,
        range_m: range,
    };
    vec![
        point(lamps[0] + Vec3::Z * 0.1, 700.0, 10.0),
        point(lamps[1] + Vec3::Z * 0.1, 700.0, 10.0),
        point(lamps[2], 500.0, 8.0),
        point(lamps[3], 250.0, 5.0),
        Light {
            position: Vec3::new(9.0, 0.3, 3.5),
            kind: LightKind::Spot {
                direction: Vec3::new(0.45, 1.0, -0.35).normalize(),
                inner: 0.3,
                outer: 0.55,
            },
            color: WARM,
            intensity: 2000.0,
            range_m: 12.0,
        },
    ]
}

pub fn garden_camera(aspect: f32) -> Camera {
    (
        Mat4::look_at_rh(Vec3::new(7.0, 2.1, 19.0), Vec3::new(0.5, 1.4, 3.0), Vec3::Y),
        Mat4::perspective_rh(55f32.to_radians(), aspect, 0.1, 1000.0),
        Vec3::new(7.0, 2.1, 19.0),
    )
}

/// A terrain chunk's data (see `terrain_chunk`).
#[derive(Clone, Debug, Default)]
pub struct TerrainData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub weights: Vec<[u8; 8]>,
}

impl TerrainData {
    pub fn chunk(&self) -> crate::TerrainChunk<'_> {
        crate::TerrainChunk {
            positions: &self.positions,
            normals: &self.normals,
            indices: &self.indices,
            weights: &self.weights,
        }
    }
}

/// Rolling hills with a pond basin around (16, 10) when `pond`.
pub fn hills_height(x: f32, z: f32, pond: bool) -> f32 {
    let mut h =
        2.2 * (x / 9.0).sin() * (z / 11.0).cos() + 0.9 * (x / 4.3 + z / 5.7).sin() + 0.02 * x;
    if pond {
        let d = ((x - 16.0) / 7.0).powi(2) + ((z - 10.0) / 5.0).powi(2);
        h = h * d.min(1.0) - 2.4 * (1.0 - d).max(0.0);
    }
    h
}

/// Layer weights for the demo terrain: grass, dirt on medium slopes and in
/// the path's joints, paving along a path, rock on steep slopes.
pub fn hills_weights(x: f32, z: f32, n: Vec3) -> [u8; 8] {
    let slope = 1.0 - n.y;
    let rock = smooth(0.12, 0.3, slope);
    let path = 1.0 - smooth(1.2, 1.8, (z - 0.35 * x - 2.0).abs());
    let dirt = smooth(0.05, 0.14, slope) * (1.0 - rock) + 0.3 * path;
    let paving = 0.7 * path;
    let grass = (1.0 - rock - dirt - paving).max(0.0);
    let w = [grass, dirt.max(0.0), paving, rock];
    let sum: f32 = w.iter().sum::<f32>().max(1e-6);
    let mut out = [0u8; 8];
    let mut left = 255i32;
    for i in 0..4 {
        let v = if i == 3 {
            left
        } else {
            (w[i] / sum * 255.0).round() as i32
        };
        let v = v.clamp(0, left);
        out[i] = v as u8;
        left -= v;
    }
    out
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One square chunk of `cells` x `cells` at `cell_m`, from `origin` (x, z),
/// keeping every `step`-th vertex (the core's LOD), with a 0.6 m skirt
/// hanging down along the border so neighbours at other LODs leave no
/// cracks. Normals come from the height function, so they agree across
/// chunk borders whatever the LOD.
pub fn terrain_chunk(
    origin: Vec2,
    cells: u32,
    cell_m: f32,
    step: u32,
    height: &dyn Fn(f32, f32) -> f32,
    weights: &dyn Fn(f32, f32, Vec3) -> [u8; 8],
) -> TerrainData {
    let n = cells / step;
    let mut t = TerrainData::default();
    let normal = |x: f32, z: f32| {
        let e = 0.1;
        Vec3::new(
            height(x - e, z) - height(x + e, z),
            2.0 * e,
            height(x, z - e) - height(x, z + e),
        )
        .normalize()
    };
    let vertex = |t: &mut TerrainData, x: f32, z: f32, drop: f32| {
        let nn = normal(x, z);
        t.positions.push([x, height(x, z) - drop, z]);
        t.normals.push(nn.to_array());
        t.weights.push(weights(x, z, nn));
        t.positions.len() as u32 - 1
    };
    let at = |i: u32, j: u32| {
        (
            origin.x + (j * step) as f32 * cell_m,
            origin.y + (i * step) as f32 * cell_m,
        )
    };
    for i in 0..=n {
        for j in 0..=n {
            let (x, z) = at(i, j);
            vertex(&mut t, x, z, 0.0);
        }
    }
    let row = n + 1;
    for i in 0..n {
        for j in 0..n {
            let a = i * row + j;
            t.indices
                .extend([a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    // Skirt: a strip down from every border edge, both windings so it shows
    // from either side.
    let border: Vec<(u32, u32)> = (0..n)
        .map(|j| (0, j))
        .chain((0..n).map(|i| (i, n)))
        .chain((0..n).map(|j| (n, n - j)))
        .chain((0..n).map(|i| (n - i, 0)))
        .collect();
    for k in 0..border.len() {
        let (i0, j0) = border[k];
        let (i1, j1) = border[(k + 1) % border.len()];
        let (a, b) = (i0 * row + j0, i1 * row + j1);
        let (x0, z0) = at(i0, j0);
        let (x1, z1) = at(i1, j1);
        let c = vertex(&mut t, x0, z0, 0.6);
        let d = vertex(&mut t, x1, z1, 0.6);
        t.indices.extend([a, c, b, b, c, d, a, b, c, b, d, c]);
    }
    t
}

/// The demo terrain's four layers: lawn, dirt, paving (with height in its
/// ORM), rock.
pub fn terrain_layers(r: &mut Renderer) {
    let lawn = lawn_texture(256).upload(r, ColorSpace::Srgb, true);
    let dirt = Image::from_fn(256, 256, |x, y| {
        let n = tile_noise(x as f32 / 3.0, y as f32 / 3.0, 85, 31) * 0.6
            + tile_noise(x as f32 / 20.0, y as f32 / 20.0, 12, 32) * 0.4;
        let c = Vec3::new(0.30, 0.24, 0.17) * (0.7 + 0.5 * n);
        [c.x, c.y, c.z, 1.0]
    })
    .upload(r, ColorSpace::Srgb, true);
    let (pave, pave_n) = paving_textures(512);
    let pave = pave.upload(r, ColorSpace::Srgb, true);
    let pave_n = pave_n.upload(r, ColorSpace::Linear, true);
    let pave_orm = paving_orm(512).upload(r, ColorSpace::Linear, true);
    let stone = Image::from_fn(256, 256, |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        let n = tile_noise(fx / 6.0, fy / 6.0, 42, 41) * 0.5
            + tile_noise(fx / 24.0, fy / 24.0, 10, 42) * 0.35
            + tile_noise(fx / 1.5, fy / 1.5, 170, 43) * 0.15;
        let c = Vec3::new(0.46, 0.44, 0.41) * (0.55 + 0.7 * n);
        [c.x, c.y, c.z, 1.0]
    });
    let rock_n = stone
        .normal_from_height(4.0)
        .upload(r, ColorSpace::Linear, true);
    let rock = stone.upload(r, ColorSpace::Srgb, true);
    let layer = crate::TerrainLayer::default;
    r.set_terrain_layers(&[
        crate::TerrainLayer {
            base_color_map: Some(lawn),
            scale_m: 2.0,
            variation: 0.6,
            ..layer()
        },
        crate::TerrainLayer {
            base_color_map: Some(dirt),
            scale_m: 1.5,
            ..layer()
        },
        crate::TerrainLayer {
            base_color_map: Some(pave),
            normal_map: Some(pave_n),
            orm_map: Some(pave_orm),
            scale_m: 1.0,
            roughness: 0.75,
            variation: 0.2,
            ..layer()
        },
        crate::TerrainLayer {
            base_color_map: Some(rock),
            normal_map: Some(rock_n),
            base_color: [0.75, 0.72, 0.7, 1.0],
            scale_m: 3.0,
            variation: 0.3,
            ..layer()
        },
    ])
    .expect("terrain layers");
}

/// A hilly 96 m terrain of 3 x 3 chunks (64 cells of 0.5 m), four layers,
/// the infinite ground beyond; with a pond (water at y = -0.35) when
/// `pond`. Returns the chunk meshes.
pub fn terrain(r: &mut Renderer, pond: bool) -> Vec<MeshId> {
    terrain_layers(r);
    let any = material(r, [0.5; 3], 0.0, 0.9);
    let mut ids = Vec::new();
    for cz in -1..2 {
        for cx in -1..2 {
            let origin =
                Vec2::new(cx as f32 * 32.0 - 16.0, cz as f32 * 32.0 - 16.0) + Vec2::new(16.0, 10.0);
            let data = terrain_chunk(
                origin,
                64,
                0.5,
                1,
                &|x, z| hills_height(x, z, pond),
                &hills_weights,
            );
            let id = r.add_terrain_chunk(&data.chunk()).expect("chunk");
            r.add_item(id, any, &[Mat4::IDENTITY]);
            ids.push(id);
        }
    }
    r.set_ground(Some(Ground {
        color: LAWN_ALBEDO,
        height: -1.0,
        ..Default::default()
    }));
    if pond {
        let mut water = MeshData::default();
        water.add_quad(
            [
                Vec3::new(9.5, -0.35, 5.5),
                Vec3::new(9.5, -0.35, 14.5),
                Vec3::new(22.5, -0.35, 14.5),
                Vec3::new(22.5, -0.35, 5.5),
            ],
            [[0.0; 2]; 4],
        );
        let water = add(r, &water);
        let mat = r.add_material(&Material {
            base_color: [0.02, 0.04, 0.03, 0.45],
            roughness: 0.05,
            surface: Surface::Water,
            ..Default::default()
        });
        r.add_item(water, mat, &[Mat4::IDENTITY]);
        let lods = plant_lods(&PlantShape::tree(), 4);
        let mats = plant_materials(r, Vec3::new(0.16, 0.3, 0.07), 6);
        let trees: Vec<Mat4> = [(14.0, 2.0), (22.0, 1.5), (27.0, 9.0)]
            .iter()
            .map(|&(x, z)| Mat4::from_translation(Vec3::new(x, hills_height(x, z, true), z)))
            .collect();
        add_plants(r, &lods, &mats, [20.0, 45.0, f32::INFINITY], &trees);
    }
    ids
}

pub fn terrain_camera(aspect: f32) -> Camera {
    camera(
        Vec3::new(-8.0, 6.0, 40.0),
        Vec3::new(16.0, 0.0, 10.0),
        aspect,
    )
}

/// The garden from above: an orthographic plan view, 44 m tall, north up.
pub fn garden_plan_camera(aspect: f32) -> Camera {
    let eye = Vec3::new(2.0, 80.0, 4.0);
    let h = 22.0;
    (
        Mat4::look_at_rh(eye, Vec3::new(2.0, 0.0, 4.0), Vec3::NEG_Z),
        Mat4::orthographic_rh(-h * aspect, h * aspect, -h, h, 0.1, 200.0),
        eye,
    )
}

/// The house's south elevation: orthographic, looking north, 14 m tall.
pub fn garden_elevation_camera(aspect: f32) -> Camera {
    let eye = Vec3::new(1.0, 4.0, 60.0);
    let h = 7.0;
    (
        Mat4::look_at_rh(eye, Vec3::new(1.0, 4.0, 0.0), Vec3::Y),
        Mat4::orthographic_rh(-h * aspect, h * aspect, -h, h, 0.1, 200.0),
        eye,
    )
}

/// Ground grid in the XZ plane at y = 0.
pub fn grid_lines(half: i32, step: f32, color: [u8; 4], width: f32) -> Vec<LineVertex> {
    let mut out = Vec::new();
    let e = half as f32 * step;
    for i in -half..=half {
        let t = i as f32 * step;
        for (a, b) in [([t, 0.0, -e], [t, 0.0, e]), ([-e, 0.0, t], [e, 0.0, t])] {
            out.push(LineVertex {
                position: a,
                color,
                width,
            });
            out.push(LineVertex {
                position: b,
                color,
                width,
            });
        }
    }
    out
}

/// The twelve edges of a box.
pub fn box_lines(min: Vec3, max: Vec3, color: [u8; 4], width: f32) -> Vec<LineVertex> {
    let c = |i: u32| {
        [
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        ]
    };
    let mut out = Vec::new();
    for a in 0..8u32 {
        for bit in [1, 2, 4] {
            if a & bit == 0 {
                out.push(LineVertex {
                    position: c(a),
                    color,
                    width,
                });
                out.push(LineVertex {
                    position: c(a | bit),
                    color,
                    width,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plant_lods_hit_their_triangle_budgets() {
        let lods = plant_lods(&PlantShape::shrub(), 1);
        let tris = lods.each_ref().map(PlantLod::triangles);
        assert_eq!(tris[0], 500, "{tris:?}");
        assert!(tris[1] < tris[0] / 2 && tris[2] < tris[1] / 2, "{tris:?}");
        for lod in &lods {
            assert_eq!(lod.foliage.uvs.len(), lod.foliage.positions.len());
            assert!(lod.foliage.wind.iter().all(|w| w[0] > 0.0));
        }
    }
}
