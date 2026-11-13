//! Built-in scenes made in code: the reference-image tests, the app's test
//! bed and the bench all use these.

use glam::{Mat4, Quat, Vec3};

use crate::{LineVertex, Material, MaterialId, Mesh, MeshId, Renderer, Sun};

/// An owned mesh under construction. Every box face and every sphere gets
/// its own face id, numbered in the order added.
#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub face_ids: Vec<u32>,
    next_face: u32,
}

impl MeshData {
    pub fn mesh(&self) -> Mesh<'_> {
        Mesh {
            positions: &self.positions,
            normals: &self.normals,
            indices: &self.indices,
            face_ids: Some(&self.face_ids),
        }
    }

    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    /// The face id the next added face will get.
    pub fn next_face(&self) -> u32 {
        self.next_face
    }

    fn vertex(&mut self, p: Vec3, n: Vec3) -> u32 {
        self.positions.push(p.to_array());
        self.normals.push(n.to_array());
        self.face_ids.push(self.next_face);
        self.positions.len() as u32 - 1
    }

    /// Axis-aligned box; faces in the order +X, -X, +Y, -Y, +Z, -Z.
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
                self.vertex(corner(a, b), n);
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
                self.vertex(center + n * radius, n);
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
}

fn material(r: &mut Renderer, rgb: [f32; 3], metallic: f32, roughness: f32) -> MaterialId {
    r.add_material(&Material {
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        metallic,
        roughness,
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

/// The bench's budget scene: ~200k triangles of static geometry (20 meshes
/// of boxes, like a CAD model's parts) plus 5000 instances of a 500-triangle
/// mesh, spread over a 50 m garden. Returns (static, instanced) triangles.
pub fn budget(r: &mut Renderer) -> (usize, usize) {
    ground(r, 40.0);
    let mut seed = 0x9e37_79b9_u32;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    let mut static_tris = 0;
    let colors = [
        [0.8, 0.78, 0.74],
        [0.5, 0.5, 0.52],
        [0.6, 0.45, 0.3],
        [0.7, 0.7, 0.75],
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
            let p = origin + Vec3::new(rand() * 6.0, rand() * 4.0, rand() * 6.0);
            let s = Vec3::new(0.1 + rand() * 0.6, 0.05 + rand() * 0.5, 0.1 + rand() * 0.6);
            m.add_box(p, p + s);
        }
        static_tris += m.triangles();
        let mesh = add(r, &m);
        let c = colors[part % 4];
        let mat = material(r, c, 0.0, 0.7);
        r.add_item(mesh, mat, &[Mat4::IDENTITY]);
    }
    let mut bush = MeshData::default();
    bush.add_sphere(Vec3::new(0.0, 0.5, 0.0), 0.5, 25, 10);
    let per = bush.triangles();
    let bush = add(r, &bush);
    let green = material(r, [0.12, 0.3, 0.08], 0.0, 0.9);
    let bushes: Vec<Mat4> = (0..5000)
        .map(|i| {
            let (x, z) = ((i % 71) as f32 * 0.7 - 24.5, (i / 71) as f32 * 0.7 - 24.5);
            let s = 0.3 + rand() * 0.4;
            Mat4::from_scale_rotation_translation(
                Vec3::new(s, s * (0.8 + rand() * 0.6), s),
                Quat::from_rotation_y(rand() * std::f32::consts::TAU),
                Vec3::new(x + rand() * 0.3, 0.0, z + rand() * 0.3),
            )
        })
        .collect();
    r.add_item(bush, green, &bushes);
    (static_tris, per * bushes.len())
}
