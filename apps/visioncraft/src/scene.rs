//! Turns the core `Project` plus loaded geometry into renderer meshes, materials and items, and
//! updates only what changed. The only place that builds `visioncraft_render::Mesh`/`Material`.

use glam::{Mat4, Vec3};
use std::collections::HashMap;
use std::sync::Arc;
use visioncraft_core::{
    Asset, Generator, ImportedModel, Material, ModelRef, Project, Shape, SurfaceKind, evaluate,
    evaluate_surface_fill, ground_height, raycast, resolve_materials, shape_mesh,
};
use visioncraft_render::{ItemId, LineVertex, MaterialId, MeshId, Renderer};

use crate::placeholder::{self, MeshData};

/// A loaded or generated design, with a counter that changes when its triangles do.
#[derive(Clone)]
pub struct Loaded {
    pub model: Arc<ImportedModel>,
    pub version: u64,
}

pub type LookKey = [u32; 9];

/// What a surface looks like to the renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    /// The artist controls of a plain PBR material.
    pub relief: f32,
    pub gloss: f32,
    pub weathering: f32,
}

impl Look {
    pub const fn plain(r: f32, g: f32, b: f32, roughness: f32) -> Self {
        Self {
            color: [r, g, b, 1.0],
            metallic: 0.0,
            roughness,
            relief: 0.0,
            gloss: 0.0,
            weathering: 0.0,
        }
    }

    pub fn key(&self) -> LookKey {
        let c = self.color;
        [
            c[0],
            c[1],
            c[2],
            c[3],
            self.metallic,
            self.roughness,
            self.relief,
            self.gloss,
            self.weathering,
        ]
        .map(f32::to_bits)
    }
}

const CLAY: Look = Look::plain(0.85, 0.85, 0.85, 0.9);
const UNASSIGNED: Look = Look::plain(0.6, 0.6, 0.6, 0.8);

fn look_of(m: &Material) -> Look {
    let [r, g, b, a] = m.base_color;
    let plain = Look {
        color: m.base_color,
        metallic: m.metallic,
        roughness: m.roughness,
        ..Look::plain(0.0, 0.0, 0.0, 0.0)
    };
    match &m.kind {
        // shortcut: glass and water are plain PBR until the app uses the renderer's glass and water surfaces
        SurfaceKind::ArchitecturalGlass { .. } => Look {
            color: [r * 0.8, g * 0.95, b, a],
            ..plain
        },
        SurfaceKind::AnimatedWater { tint, .. } => Look {
            color: [tint[0] * 0.7, tint[1] + 0.05, tint[2] + 0.15, 1.0],
            roughness: 0.08,
            ..plain
        },
        SurfaceKind::StandardTriplanarPbr {
            relief,
            gloss,
            weathering,
            ..
        } => Look {
            relief: *relief,
            gloss: *gloss,
            weathering: *weathering,
            ..plain
        },
        SurfaceKind::Lawn3d(_) => plain,
    }
}

/// The renderer material of a look: the one place a `visioncraft_render::Material` is built.
#[allow(clippy::needless_update)] // the renderer's Material will gain fields
fn gpu_material(look: &Look) -> visioncraft_render::Material {
    visioncraft_render::Material {
        base_color: look.color,
        metallic: look.metallic,
        roughness: look.roughness,
        relief: look.relief,
        gloss: look.gloss,
        weathering: look.weathering,
        ..Default::default()
    }
}

/// The one place a `visioncraft_render::Mesh` is built.
fn upload(r: &mut Renderer, d: &MeshData, face_ids: bool) -> Option<MeshId> {
    let mesh = visioncraft_render::Mesh {
        positions: &d.positions,
        normals: &d.normals,
        indices: &d.indices,
        face_ids: face_ids.then_some(d.face_ids.as_slice()),
        uvs: None,
        wind: None,
    };
    r.add_mesh(&mesh).ok()
}

#[derive(Clone, Debug, PartialEq)]
pub enum Geom {
    Model(u64),
    Shape(Box<Shape>),
}

/// Everything that decides the triangles and materials of an owner's pieces.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnerKey {
    pub geom: Geom,
    pub visible: bool,
    pub looks: Vec<LookKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Keep,
    Transform,
    Rebuild,
}

/// What a changed owner needs: nothing, only new instance matrices, or its pieces rebuilt.
pub fn classify(old: &OwnerKey, new: &OwnerKey, old_xf: &Mat4, new_xf: &Mat4) -> Change {
    if old != new {
        Change::Rebuild
    } else if old_xf != new_xf {
        Change::Transform
    } else {
        Change::Keep
    }
}

struct Piece {
    mesh: MeshId,
    item: ItemId,
}

struct Owner {
    key: OwnerKey,
    xf: Mat4,
    geom: Arc<ImportedModel>,
    pieces: Vec<Piece>,
    /// Per face of `geom`: index into `pieces`.
    face_piece: Vec<Option<usize>>,
    is_shape: bool,
    is_area: bool,
}

type Wanted = (String, Geom, Arc<ImportedModel>, Mat4, bool, bool, bool);

struct GenGpu {
    key: (Generator, u64, bool),
    items: Vec<ItemId>,
}

/// The model transform: scale, then rotation about +Y, then translation.
pub fn model_matrix(m: &ModelRef) -> Mat4 {
    let t = m.transform;
    Mat4::from_translation(Vec3::from(t.translation))
        * Mat4::from_rotation_y(t.rotation_y_deg.to_radians())
        * Mat4::from_scale(Vec3::splat(t.scale))
}

#[derive(Default)]
pub struct Scene {
    owners: HashMap<String, Owner>,
    gens: HashMap<String, GenGpu>,
    materials: HashMap<LookKey, MaterialId>,
    protos: HashMap<String, Vec<(MeshId, MaterialId)>>,
    /// Bumps whenever any owner's geometry or placement changes: generators depend on the ground.
    ground_rev: u64,
}

#[derive(Clone, Debug)]
pub struct Pick {
    pub owner: String,
    pub face: usize,
    pub face_name: String,
    pub body: String,
    pub point: Vec3,
    pub distance: f32,
    pub is_shape: bool,
}

fn world_bounds(model: &ImportedModel, xf: &Mat4) -> Option<(Vec3, Vec3)> {
    let mut b: Option<(Vec3, Vec3)> = None;
    for body in &model.bodies {
        for i in 0..8 {
            let p = |k: usize, bit: usize| body.bounds[(i >> bit) & 1][k];
            let w = xf.transform_point3(Vec3::new(p(0, 0), p(1, 1), p(2, 2)));
            b = Some(b.map_or((w, w), |(lo, hi)| (lo.min(w), hi.max(w))));
        }
    }
    b
}

impl Scene {
    fn material(&mut self, r: &mut Renderer, look: &Look) -> MaterialId {
        *self
            .materials
            .entry(look.key())
            .or_insert_with(|| r.add_material(&gpu_material(look)))
    }

    fn drop_owner(r: &mut Renderer, o: Owner) {
        for p in o.pieces {
            r.remove_item(p.item);
            r.remove_mesh(p.mesh);
        }
    }

    pub fn clear(&mut self, r: &mut Renderer) {
        for (_, o) in self.owners.drain() {
            Self::drop_owner(r, o);
        }
        for (_, g) in self.gens.drain() {
            g.items.into_iter().for_each(|i| r.remove_item(i));
        }
        self.ground_rev += 1;
    }

    /// Brings the renderer in line with `project`. Returns plain-words problems (a shape that
    /// does not mesh, a mesh the GPU refused).
    pub fn sync(
        &mut self,
        r: &mut Renderer,
        project: &Project,
        loaded: &HashMap<String, Loaded>,
        assets: &[Asset],
    ) -> Vec<String> {
        let mut problems = Vec::new();
        let mut live: Vec<String> = Vec::new();
        let clay = project.style.clay;

        // Owners: models, then shapes.
        let mut wanted: Vec<Wanted> = Vec::new();
        for m in &project.models {
            if let Some(l) = loaded.get(&m.id) {
                let visible = project.layer_visible(&m.layer);
                wanted.push((
                    m.id.clone(),
                    Geom::Model(l.version),
                    l.model.clone(),
                    model_matrix(m),
                    visible,
                    false,
                    false,
                ));
            }
        }
        for s in &project.shapes {
            let id = s.id().to_string();
            let geom = Geom::Shape(Box::new(s.clone()));
            let visible = project.layer_visible(s.layer());
            // The mesh is only rebuilt when the shape changed.
            let model = match self.owners.get(&id) {
                Some(o) if o.key.geom == geom => o.geom.clone(),
                _ => match shape_mesh(s) {
                    Ok(m) => Arc::new(m),
                    Err(e) => {
                        problems.push(format!("{}: {e}", s.name()));
                        if self.owners.contains_key(&id) {
                            live.push(id); // the last good mesh stays
                        }
                        continue;
                    }
                },
            };
            let is_area = matches!(s, Shape::Area(_));
            wanted.push((id, geom, model, Mat4::IDENTITY, visible, true, is_area));
        }

        for (id, geom, model, xf, visible, is_shape, is_area) in wanted {
            live.push(id.clone());
            let owner_looks = |project: &Project| -> Vec<Look> {
                let resolved = resolve_materials(project, &id, &model);
                model
                    .faces
                    .iter()
                    .zip(resolved)
                    .map(|(f, m)| {
                        if clay {
                            CLAY
                        } else if let Some(i) = m {
                            look_of(&project.materials[i])
                        } else if let Some(c) = model.bodies[f.body as usize].color {
                            Look {
                                color: c,
                                roughness: 0.7,
                                ..Look::plain(0.0, 0.0, 0.0, 0.0)
                            }
                        } else {
                            UNASSIGNED
                        }
                    })
                    .collect()
            };
            let looks = owner_looks(project);
            let key = OwnerKey {
                geom,
                visible,
                looks: looks.iter().map(Look::key).collect(),
            };
            let change = match self.owners.get(&id) {
                Some(o) => classify(&o.key, &key, &o.xf, &xf),
                None => Change::Rebuild,
            };
            match change {
                Change::Keep => {}
                Change::Transform => {
                    let o = self.owners.get_mut(&id).expect("classified from it");
                    o.xf = xf;
                    for p in &o.pieces {
                        r.set_instances(p.item, &[xf]);
                    }
                    self.ground_rev += 1;
                }
                Change::Rebuild => {
                    if let Some(old) = self.owners.remove(&id) {
                        Self::drop_owner(r, old);
                    }
                    let (pieces, face_piece) = if visible {
                        self.build_pieces(r, &model, &looks, xf, &mut problems)
                    } else {
                        (Vec::new(), vec![None; model.faces.len()])
                    };
                    self.owners.insert(
                        id.clone(),
                        Owner {
                            key,
                            xf,
                            geom: model,
                            pieces,
                            face_piece,
                            is_shape,
                            is_area,
                        },
                    );
                    self.ground_rev += 1;
                }
            }
        }
        let gone: Vec<String> = self
            .owners
            .keys()
            .filter(|k| !live.contains(k))
            .cloned()
            .collect();
        for id in gone {
            if let Some(o) = self.owners.remove(&id) {
                Self::drop_owner(r, o);
                self.ground_rev += 1;
            }
        }

        self.sync_generators(r, project, assets, clay);
        problems
    }

    fn build_pieces(
        &mut self,
        r: &mut Renderer,
        model: &ImportedModel,
        looks: &[Look],
        xf: Mat4,
        problems: &mut Vec<String>,
    ) -> (Vec<Piece>, Vec<Option<usize>>) {
        // Triangles grouped by look, in first-seen order.
        let mut groups: Vec<(Look, MeshData)> = Vec::new();
        let mut by_key: HashMap<LookKey, usize> = HashMap::new();
        let mut face_group: Vec<Option<usize>> = vec![None; model.faces.len()];
        for (t, tri) in model.triangles.iter().enumerate() {
            let face = model.triangle_faces[t] as usize;
            let look = looks[face];
            let g = *by_key.entry(look.key()).or_insert_with(|| {
                groups.push((look, MeshData::default()));
                groups.len() - 1
            });
            face_group[face] = Some(g);
            let d = &mut groups[g].1;
            for &v in tri {
                d.indices.push(d.positions.len() as u32);
                d.positions.push(model.positions[v as usize]);
                d.normals.push(model.normals[v as usize]);
                d.face_ids.push(face as u32);
            }
        }
        let mut pieces = Vec::new();
        let mut group_piece = vec![None; groups.len()];
        for (g, (look, data)) in groups.iter().enumerate() {
            let Some(mesh) = upload(r, data, true) else {
                problems.push("a mesh was too large for the GPU and is not drawn".into());
                continue;
            };
            let mat = self.material(r, look);
            let item = r.add_item(mesh, mat, &[xf]);
            group_piece[g] = Some(pieces.len());
            pieces.push(Piece { mesh, item });
        }
        (
            pieces,
            face_group
                .iter()
                .map(|g| g.and_then(|g| group_piece[g]))
                .collect(),
        )
    }

    // ---- planting ----

    fn ground_fn(&self) -> impl Fn(f32, f32) -> Option<f32> + '_ {
        // Planting stands on areas and imported models, not on the roof of a house block.
        move |x, z| {
            self.owners
                .values()
                .filter(|o| !o.is_shape || o.is_area)
                .filter(|o| o.key.visible)
                .filter_map(|o| {
                    let inv = o.xf.inverse();
                    let local = inv.transform_point3(Vec3::new(x, 0.0, z));
                    ground_height(&o.geom, local.x, local.z)
                        .map(|y| o.xf.transform_point3(Vec3::new(local.x, y, local.z)).y)
                })
                .fold(None, |a: Option<f32>, y| Some(a.map_or(y, |a| a.max(y))))
        }
    }

    fn sync_generators(
        &mut self,
        r: &mut Renderer,
        project: &Project,
        assets: &[Asset],
        clay: bool,
    ) {
        let ids: Vec<&str> = project.generators.iter().map(Generator::id).collect();
        let gone: Vec<String> = self
            .gens
            .keys()
            .filter(|k| !ids.contains(&k.as_str()))
            .cloned()
            .collect();
        for id in gone {
            if let Some(g) = self.gens.remove(&id) {
                g.items.into_iter().for_each(|i| r.remove_item(i));
            }
        }
        for g in &project.generators {
            let key = (g.clone(), self.ground_rev, clay);
            if self.gens.get(g.id()).is_some_and(|old| old.key == key) {
                continue;
            }
            if let Some(old) = self.gens.remove(g.id()) {
                old.items.into_iter().for_each(|i| r.remove_item(i));
            }
            let mut items = Vec::new();
            if project.layer_visible(g.layer()) {
                for (asset, matrices) in self.placements(g) {
                    let asset_def = assets.iter().find(|a| a.id == asset);
                    for (mesh, mat) in self.proto(r, &asset, asset_def, clay) {
                        items.push(r.add_item(mesh, mat, &matrices));
                    }
                }
            }
            self.gens.insert(g.id().to_string(), GenGpu { key, items });
        }
    }

    /// Instance matrices per asset id for one generator.
    fn placements(&self, g: &Generator) -> Vec<(String, Vec<Mat4>)> {
        let placements = match g {
            Generator::SurfaceFill(f) => match self.owners.get(&f.owner) {
                Some(o) => evaluate_surface_fill(f, &o.geom)
                    .placements
                    .into_iter()
                    .map(|mut p| {
                        p.position = o.xf.transform_point3(Vec3::from(p.position)).to_array();
                        p
                    })
                    .collect(),
                None => Vec::new(),
            },
            _ => evaluate(g, &self.ground_fn()).placements,
        };
        let mut out: Vec<(String, Vec<Mat4>)> = Vec::new();
        for p in placements {
            let m = Mat4::from_translation(Vec3::from(p.position))
                * Mat4::from_rotation_y(p.rotation_y_deg.to_radians())
                * Mat4::from_scale(Vec3::splat(p.scale));
            match out.iter_mut().find(|(a, _)| *a == p.asset) {
                Some((_, v)) => v.push(m),
                None => out.push((p.asset, vec![m])),
            }
        }
        out
    }

    /// Placeholder meshes of an asset, built once per asset and clay setting.
    fn proto(
        &mut self,
        r: &mut Renderer,
        id: &str,
        asset: Option<&Asset>,
        clay: bool,
    ) -> Vec<(MeshId, MaterialId)> {
        let cache_key = format!("{id}{}", if clay { "#clay" } else { "" });
        if let Some(p) = self.protos.get(&cache_key) {
            return p.clone();
        }
        let mut out = Vec::new();
        for (data, look) in placeholder::parts(asset) {
            let look = if clay { CLAY } else { look };
            if let Some(mesh) = upload(r, &data, false) {
                out.push((mesh, self.material(r, &look)));
            }
        }
        self.protos.insert(cache_key, out.clone());
        out
    }

    // ---- queries ----

    pub fn raycast(&self, origin: Vec3, dir: Vec3, shapes_only: bool) -> Option<Pick> {
        let mut best: Option<Pick> = None;
        for (id, o) in &self.owners {
            if !o.key.visible || (shapes_only && !o.is_shape) {
                continue;
            }
            let inv = o.xf.inverse();
            let (lo, ld) = (inv.transform_point3(origin), inv.transform_vector3(dir));
            let Some(h) = raycast(&o.geom, lo.to_array(), ld.to_array()) else {
                continue;
            };
            if best.as_ref().is_some_and(|b| b.distance <= h.distance) {
                continue;
            }
            let face = &o.geom.faces[h.face];
            best = Some(Pick {
                owner: id.clone(),
                face: h.face,
                face_name: face.name.clone(),
                body: o.geom.bodies[face.body as usize].id.clone(),
                point: origin + dir * h.distance,
                distance: h.distance,
                is_shape: o.is_shape,
            });
        }
        best
    }

    /// Height of the floor under `(x, z)`, looking down from `from_y`.
    pub fn floor_below(&self, x: f32, from_y: f32, z: f32) -> Option<f32> {
        self.raycast(Vec3::new(x, from_y, z), Vec3::NEG_Y, false)
            .map(|h| h.point.y)
    }

    pub fn highlight(&self, owner: &str, face: usize) -> Option<(MeshId, u32)> {
        let o = self.owners.get(owner)?;
        let piece = (*o.face_piece.get(face)?)?;
        Some((o.pieces[piece].mesh, face as u32))
    }

    /// Geometry of an owner, for tools that need its outline or faces.
    pub fn geometry(&self, owner: &str) -> Option<&Arc<ImportedModel>> {
        self.owners.get(owner).map(|o| &o.geom)
    }

    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        self.owners
            .values()
            .filter(|o| o.key.visible)
            .filter_map(|o| world_bounds(&o.geom, &o.xf))
            .reduce(|(a, b), (c, d)| (a.min(c), b.max(d)))
    }
}

pub fn line(a: Vec3, b: Vec3, color: [u8; 4], width: f32) -> [LineVertex; 2] {
    [a, b].map(|p| LineVertex {
        position: p.to_array(),
        color,
        width,
    })
}

/// Segments of the twelve edges of a box moved by `xf`.
pub fn box_outline(min: Vec3, max: Vec3, xf: &Mat4, color: [u8; 4], width: f32) -> Vec<LineVertex> {
    let c = |i: usize| {
        xf.transform_point3(Vec3::new(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        ))
    };
    let mut out = Vec::new();
    for a in 0..8usize {
        for bit in [1, 2, 4] {
            if a & bit == 0 {
                out.extend(line(c(a), c(a | bit), color, width));
            }
        }
    }
    out
}

/// A small three-axis cross for every enabled light on a visible layer.
pub fn light_markers(project: &Project) -> Vec<LineVertex> {
    let mut out = Vec::new();
    for l in project
        .lights
        .iter()
        .filter(|l| l.enabled && project.layer_visible(&l.layer))
    {
        let p = Vec3::from(l.position);
        let c = [255, 214, 120, 255];
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            out.extend(line(p - axis * 0.2, p + axis * 0.2, c, 2.0));
        }
        out.extend(line(p, p, c, 7.0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(version: u64, looks: Vec<LookKey>, visible: bool) -> OwnerKey {
        OwnerKey {
            geom: Geom::Model(version),
            visible,
            looks,
        }
    }

    #[test]
    fn classify_picks_the_cheapest_update() {
        let a = key(1, vec![[0; 9]], true);
        let (i, t) = (Mat4::IDENTITY, Mat4::from_translation(Vec3::X));
        assert_eq!(classify(&a, &a.clone(), &i, &i), Change::Keep);
        assert_eq!(classify(&a, &a.clone(), &i, &t), Change::Transform);
        assert_eq!(
            classify(&a, &key(2, vec![[0; 9]], true), &i, &i),
            Change::Rebuild
        );
        assert_eq!(
            classify(&a, &key(1, vec![[1; 9]], true), &i, &i),
            Change::Rebuild
        );
        assert_eq!(
            classify(&a, &key(1, vec![[0; 9]], false), &i, &t),
            Change::Rebuild
        );
    }

    #[test]
    fn model_matrix_scales_then_rotates_then_moves() {
        let m = ModelRef {
            id: "m1".into(),
            name: "m".into(),
            kind: visioncraft_core::ModelKind::Obj,
            source: String::new(),
            transform: visioncraft_core::Transform {
                translation: [1.0, 0.0, 0.0],
                rotation_y_deg: 90.0,
                scale: 2.0,
            },
            layer: "model".into(),
            extensions: Default::default(),
        };
        let p = model_matrix(&m).transform_point3(Vec3::X);
        assert!((p - Vec3::new(1.0, 0.0, -2.0)).length() < 1e-5, "{p}");
    }

    #[test]
    fn equal_looks_share_a_key() {
        assert_eq!(CLAY.key(), CLAY.key());
        assert_ne!(CLAY.key(), UNASSIGNED.key());
    }
}
