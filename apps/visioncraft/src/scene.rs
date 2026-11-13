//! Turns the core `Project` plus loaded geometry into renderer meshes, materials and items, and
//! updates only what changed. The only place that builds `visioncraft_render::Mesh`/`Material`.

use glam::{Mat4, Vec3};
use std::collections::HashMap;
use std::sync::Arc;
use visioncraft_core::{
    Asset, Evaluation, Generator, ImportedModel, Material, ModelRef, Project, Shape, SurfaceKind,
    Terrain, evaluate, evaluate_surface_fill, ground_height, raycast, resolve_materials,
    shape_mesh,
};
use visioncraft_flora as flora;
use visioncraft_render::{ItemId, LineVertex, Renderer};
pub use visioncraft_render::{MaterialId, MeshId};

use crate::placeholder::{self, MeshData};
use crate::plant_scene::{self, DirtyLog, EVERYTHING, Inst, Rect};
use crate::plants::{self, Flora};

/// A loaded or generated design, with a counter that changes when its triangles do.
#[derive(Clone)]
pub struct Loaded {
    pub model: Arc<ImportedModel>,
    pub version: u64,
}

pub type LookKey = [u32; 12];

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
    /// 0 standard, 1 glass, 2 water.
    pub surface: u32,
    pub variation: f32,
    /// Metres per texture repeat.
    pub scale_m: f32,
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
            surface: 0,
            variation: 0.0,
            scale_m: 1.0,
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
            self.surface as f32,
            self.variation,
            self.scale_m,
        ]
        .map(f32::to_bits)
    }
}

const CLAY: Look = Look::plain(0.85, 0.85, 0.85, 0.9);
const UNASSIGNED: Look = Look::plain(0.6, 0.6, 0.6, 0.8);

fn look_of(m: &Material) -> Look {
    let plain = Look {
        color: m.base_color,
        metallic: m.metallic,
        roughness: m.roughness,
        ..Look::plain(0.0, 0.0, 0.0, 0.0)
    };
    match &m.kind {
        SurfaceKind::ArchitecturalGlass { tint, roughness } => Look {
            color: [tint[0], tint[1], tint[2], 0.25],
            roughness: *roughness,
            surface: 1,
            ..plain
        },
        SurfaceKind::AnimatedWater { tint, .. } => Look {
            color: [tint[0] * 0.25, tint[1] * 0.25, tint[2] * 0.25, 0.3],
            roughness: 0.05,
            surface: 2,
            ..plain
        },
        SurfaceKind::StandardTriplanarPbr {
            map_scale_m,
            relief,
            gloss,
            weathering,
        } => Look {
            relief: *relief,
            gloss: *gloss,
            weathering: *weathering,
            scale_m: *map_scale_m,
            ..plain
        },
        // shortcut: 3D grass is not scattered yet; the lawn is a green surface with large-scale variation
        SurfaceKind::Lawn3d(_) => Look {
            variation: 0.6,
            ..plain
        },
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
        variation: look.variation,
        projection: visioncraft_render::Projection::Triplanar {
            scale_m: look.scale_m.max(0.01),
        },
        surface: match look.surface {
            1 => visioncraft_render::Surface::Glass,
            2 => visioncraft_render::Surface::Water,
            _ => visioncraft_render::Surface::Standard,
        },
        ..Default::default()
    }
}

/// A plant mesh part (positions, normals, UVs, wind) into the renderer.
pub fn upload_plant_mesh(r: &mut Renderer, p: &flora::MeshPart) -> Option<MeshId> {
    let mesh = visioncraft_render::Mesh {
        positions: &p.positions,
        normals: &p.normals,
        indices: &p.indices,
        face_ids: None,
        uvs: Some(&p.uvs),
        wind: Some(&p.wind),
    };
    r.add_mesh(&mesh).ok()
}

pub fn upload_texture(
    r: &mut Renderer,
    w: u32,
    h: u32,
    rgba: &[u8],
    srgb: bool,
    repeat: bool,
) -> Option<visioncraft_render::TextureId> {
    r.add_texture(&visioncraft_render::TextureDesc {
        width: w,
        height: h,
        rgba8: rgba,
        color_space: if srgb {
            visioncraft_render::ColorSpace::Srgb
        } else {
            visioncraft_render::ColorSpace::Linear
        },
        repeat,
    })
    .ok()
}

/// Material of one plant part: bark with its normal map, cutout double-sided foliage with the season's tint.
pub fn plant_material(
    r: &mut Renderer,
    kind: flora::Material,
    tex: (
        visioncraft_render::TextureId,
        visioncraft_render::TextureId,
        visioncraft_render::TextureId,
    ),
    tint: [f32; 3],
) -> MaterialId {
    use visioncraft_render::{AlphaMode, Material as M};
    let (atlas, bark, bark_normal) = tex;
    let leaf = |color: [f32; 4], foliage: bool| M {
        base_color: color,
        base_color_map: Some(atlas),
        roughness: 0.6,
        alpha_mode: AlphaMode::Cutout { threshold: 0.5 },
        double_sided: true,
        foliage,
        ..Default::default()
    };
    let m = match kind {
        flora::Material::Bark => M {
            base_color_map: Some(bark),
            normal_map: Some(bark_normal),
            roughness: 0.9,
            ..Default::default()
        },
        flora::Material::Foliage => leaf([tint[0], tint[1], tint[2], 1.0], true),
        flora::Material::Flower => leaf([1.0; 4], true),
        flora::Material::Twigs => leaf([1.0; 4], false),
    };
    r.add_material(&m)
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

/// The instances of one (asset, variant) of a generator: its renderer items (a plant is one LOD
/// item, a stand-in several), whether they are the real plant, and the matrices they draw.
struct Group {
    items: Vec<ItemId>,
    real: bool,
    matrices: Vec<Mat4>,
}

struct GenGpu {
    /// What the evaluation depended on besides the ground: the generator, the month, visibility.
    sig: (Generator, u8, bool),
    /// The dirty-log stamp it was evaluated at, and where it puts plants.
    stamp: u64,
    bounds: Option<Rect>,
    flora_rev: u64,
    groups: HashMap<(String, u32), Group>,
    insts: Vec<Inst>,
    truncated: bool,
}

/// Plants of one generator, as the panel shows them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GenStats {
    pub count: usize,
    pub truncated: bool,
}

/// Milliseconds the last generator evaluation and its upload to the renderer took.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timings {
    pub eval_ms: f64,
    pub upload_ms: f64,
    pub placements: usize,
    /// How many generator evaluations have run in all.
    pub rebuilds: u64,
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
    /// Where the ground changed since which stamp: generators depend on it.
    dirty: DirtyLog,
    terrain: Option<Arc<Terrain>>,
    pub timings: Timings,
    pub flora: Flora,
    people: Option<ItemId>,
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
            Self::drop_groups(r, g.groups);
        }
        self.dirty.mark(EVERYTHING);
    }

    fn drop_groups(r: &mut Renderer, groups: HashMap<(String, u32), Group>) {
        for g in groups.into_values() {
            g.items.into_iter().for_each(|i| r.remove_item(i));
        }
    }

    /// The x/z extent of an owner (any owner: a surface fill follows walls too).
    fn ground_rect(o: &Owner) -> Option<Rect> {
        world_bounds(&o.geom, &o.xf).map(|(lo, hi)| [[lo.x, lo.z], [hi.x, hi.z]])
    }

    fn ground_changed(&mut self, rects: [Option<Rect>; 2]) {
        if let Some(r) = rects.into_iter().flatten().reduce(plant_scene::rect_union) {
            self.dirty.mark(r);
        }
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
        // A new terrain (every terrain edit makes one) moves the ground under its whole extent.
        let same = match (&self.terrain, &project.terrain) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        if !same {
            let extents = [&self.terrain, &project.terrain].map(|t| t.as_ref().map(|t| t.extent()));
            self.ground_changed(extents);
            self.terrain.clone_from(&project.terrain);
        }
        let clay = false; // the renderer's own clay mode handles it (`Renderer::set_clay`)

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
                    let before = Self::ground_rect(o);
                    o.xf = xf;
                    for p in &o.pieces {
                        r.set_instances(p.item, &[xf]);
                    }
                    let after = Self::ground_rect(o);
                    self.ground_changed([before, after]);
                }
                Change::Rebuild => {
                    let mut before = None;
                    if let Some(old) = self.owners.remove(&id) {
                        before = Self::ground_rect(&old);
                        Self::drop_owner(r, old);
                    }
                    let (pieces, face_piece) = if visible {
                        self.build_pieces(r, &model, &looks, xf, &mut problems)
                    } else {
                        (Vec::new(), vec![None; model.faces.len()])
                    };
                    let owner = Owner {
                        key,
                        xf,
                        geom: model,
                        pieces,
                        face_piece,
                        is_shape,
                        is_area,
                    };
                    let after = Self::ground_rect(&owner);
                    self.owners.insert(id.clone(), owner);
                    self.ground_changed([before, after]);
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
                let before = Self::ground_rect(&o);
                Self::drop_owner(r, o);
                self.ground_changed([before, None]);
            }
        }

        self.sync_generators(r, project, assets);
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

    /// The ground plants stand on: the terrain, areas and imported models, the highest wins.
    pub fn ground_fn(&self) -> impl Fn(f32, f32) -> Option<f32> + '_ {
        // Planting stands on areas and imported models, not on the roof of a house block.
        move |x, z| {
            let terrain = self.terrain.as_ref().and_then(|t| t.height_at(x, z));
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
                .fold(terrain, |a: Option<f32>, y| Some(a.map_or(y, |a| a.max(y))))
        }
    }

    fn sync_generators(&mut self, r: &mut Renderer, project: &Project, assets: &[Asset]) {
        let ids: Vec<&str> = project.generators.iter().map(Generator::id).collect();
        let gone: Vec<String> = self
            .gens
            .keys()
            .filter(|k| !ids.contains(&k.as_str()))
            .cloned()
            .collect();
        for id in gone {
            if let Some(g) = self.gens.remove(&id) {
                Self::drop_groups(r, g.groups);
            }
        }
        let month = project.time.month as u8;
        for g in &project.generators {
            let sig = (g.clone(), month, project.layer_visible(g.layer()));
            let bounds = match g {
                Generator::SurfaceFill(f) => self
                    .owners
                    .get(&f.owner)
                    .and_then(|o| world_bounds(&o.geom, &o.xf))
                    .map(|(lo, hi)| [[lo.x, lo.z], [hi.x, hi.z]]),
                _ => plant_scene::generator_bounds(g),
            };
            let mut old = self.gens.remove(g.id());
            if let Some(o) = &mut old
                && o.sig == sig
                && !self.dirty.touches(o.stamp, o.bounds)
            {
                if o.flora_rev != self.flora.rev {
                    self.upgrade(r, project, &mut o.groups);
                    o.flora_rev = self.flora.rev;
                }
                if let Some(o) = old {
                    self.gens.insert(g.id().to_string(), o);
                }
                continue;
            }
            let built = self.rebuild(r, project, assets, g, sig, bounds, old);
            self.gens.insert(g.id().to_string(), built);
        }
    }

    /// Swaps stand-ins for the real plant once its generation finished.
    fn upgrade(
        &mut self,
        r: &mut Renderer,
        project: &Project,
        groups: &mut HashMap<(String, u32), Group>,
    ) {
        let month = project.time.month as u8;
        for ((asset, variant), grp) in groups.iter_mut().filter(|(_, g)| !g.real) {
            let Some(res) = plants::resolve(project, asset) else {
                continue;
            };
            let class = plants::class(&res.params, month);
            if let Some(parts) = self.flora.parts(asset, class, *variant) {
                grp.items.drain(..).for_each(|i| r.remove_item(i));
                grp.items.push(r.add_lod_item(parts, &grp.matrices));
                grp.real = true;
            }
        }
    }

    /// Evaluates one generator and brings its renderer items in line: instance lists per
    /// (asset, variant) are updated in place, items are only created or removed for groups that
    /// appear or vanish.
    #[allow(clippy::too_many_arguments)]
    fn rebuild(
        &mut self,
        r: &mut Renderer,
        project: &Project,
        assets: &[Asset],
        g: &Generator,
        sig: (Generator, u8, bool),
        bounds: Option<Rect>,
        old: Option<GenGpu>,
    ) -> GenGpu {
        let t0 = plant_scene::now_ms();
        let (placements, truncated) = if sig.2 {
            let e = self.evaluate_generator(g);
            (e.placements, e.truncated)
        } else {
            (Vec::new(), false)
        };
        let t1 = plant_scene::now_ms();
        let month = sig.1;
        let stamp = self.dirty.stamp();
        let mut old_groups = old.map(|o| o.groups).unwrap_or_default();
        let mut groups: HashMap<(String, u32), Group> = HashMap::new();
        for (asset, variant, matrices) in
            plant_scene::group_placements(&placements, plants::VARIANTS)
        {
            let res = plants::resolve(project, &asset);
            let class = res.as_ref().map(|res| plants::class(&res.params, month));
            let real = class.is_some_and(|c| self.flora.parts(&asset, c, variant).is_some());
            let items = match old_groups.remove(&(asset.clone(), variant)) {
                Some(prev) if prev.real == real => {
                    prev.items
                        .iter()
                        .for_each(|i| r.set_instances(*i, &matrices));
                    prev.items
                }
                prev => {
                    if let Some(p) = prev {
                        p.items.into_iter().for_each(|i| r.remove_item(i));
                    }
                    let mut items = Vec::new();
                    match (res, class) {
                        (Some(_), Some(c)) if real => {
                            let parts = self.flora.parts(&asset, c, variant).expect("checked");
                            items.push(r.add_lod_item(parts, &matrices));
                        }
                        (Some(res), Some(c)) => {
                            self.flora.want(&asset, c, month, &res.params);
                            let stand_in = placeholder::flora_asset(&asset, &res);
                            for (mesh, mat) in self.proto(r, &asset, Some(&stand_in)) {
                                items.push(r.add_item(mesh, mat, &matrices));
                            }
                        }
                        _ => {
                            let def = assets.iter().find(|a| a.id == asset);
                            for (mesh, mat) in self.proto(r, &asset, def) {
                                items.push(r.add_item(mesh, mat, &matrices));
                            }
                        }
                    }
                    items
                }
            };
            groups.insert(
                (asset, variant),
                Group {
                    items,
                    real,
                    matrices,
                },
            );
        }
        Self::drop_groups(r, old_groups);
        let insts = plant_scene::instances(project, assets, &placements);
        self.timings = Timings {
            eval_ms: t1 - t0,
            upload_ms: plant_scene::now_ms() - t1,
            placements: placements.len(),
            rebuilds: self.timings.rebuilds + 1,
        };
        GenGpu {
            sig,
            stamp,
            bounds,
            flora_rev: self.flora.rev,
            groups,
            insts,
            truncated,
        }
    }

    /// One step of lazy plant generation; true when a generation finished (the scene must sync again).
    pub fn pump(&mut self, r: &mut Renderer) -> bool {
        self.flora.pump(r)
    }

    /// The placements of a generator against the current ground (a surface fill against its
    /// owner's mesh), in world space. Also what the tools preview.
    pub fn evaluate_generator(&self, g: &Generator) -> Evaluation {
        match g {
            Generator::SurfaceFill(f) => match self.owners.get(&f.owner) {
                Some(o) => {
                    let e = evaluate_surface_fill(f, &o.geom);
                    Evaluation {
                        placements: e
                            .placements
                            .into_iter()
                            .map(|mut p| {
                                p.position =
                                    o.xf.transform_point3(Vec3::from(p.position)).to_array();
                                p
                            })
                            .collect(),
                        truncated: e.truncated,
                    }
                }
                None => Evaluation {
                    placements: Vec::new(),
                    truncated: false,
                },
            },
            _ => evaluate(g, &self.ground_fn()),
        }
    }

    pub fn gen_stats(&self, id: &str) -> Option<GenStats> {
        let g = self.gens.get(id)?;
        Some(GenStats {
            count: g.insts.len(),
            truncated: g.truncated,
        })
    }

    /// The generator whose plant the ray hits first (against bounding cylinders), and the distance.
    pub fn pick_plant(&self, origin: Vec3, dir: Vec3) -> Option<(String, f32)> {
        self.pick_plant_detail(origin, dir)
            .map(|(id, _, t)| (id, t))
    }

    /// Like `pick_plant`, with the plant's bounding cylinder.
    pub fn pick_plant_detail(&self, origin: Vec3, dir: Vec3) -> Option<(String, Inst, f32)> {
        self.gens
            .iter()
            .filter_map(|(id, g)| {
                plant_scene::pick_instance(&g.insts, origin, dir)
                    .map(|(i, t)| (id.clone(), g.insts[i], t))
            })
            .min_by(|a, b| a.2.total_cmp(&b.2))
    }

    /// Placeholder meshes of an asset, built once per asset.
    fn proto(
        &mut self,
        r: &mut Renderer,
        id: &str,
        asset: Option<&Asset>,
    ) -> Vec<(MeshId, MaterialId)> {
        if let Some(p) = self.protos.get(id) {
            return p.clone();
        }
        let mut out = Vec::new();
        for (data, look) in placeholder::parts(asset) {
            if let Some(mesh) = upload(r, &data, false) {
                out.push((mesh, self.material(r, &look)));
            }
        }
        self.protos.insert(id.to_string(), out.clone());
        out
    }

    /// The scale figure(s): one shared mesh, one instance per figure.
    pub fn set_people(&mut self, r: &mut Renderer, at: &[Mat4]) {
        match self.people {
            Some(item) => r.set_instances(item, at),
            None if at.is_empty() => {}
            None => {
                let data = placeholder::person();
                if let Some(mesh) = upload(r, &data, false) {
                    let mat = self.material(r, &Look::plain(0.05, 0.05, 0.06, 0.9));
                    self.people = Some(r.add_item(mesh, mat, at));
                }
            }
        }
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
        let a = key(1, vec![[0; 12]], true);
        let (i, t) = (Mat4::IDENTITY, Mat4::from_translation(Vec3::X));
        assert_eq!(classify(&a, &a.clone(), &i, &i), Change::Keep);
        assert_eq!(classify(&a, &a.clone(), &i, &t), Change::Transform);
        assert_eq!(
            classify(&a, &key(2, vec![[0; 12]], true), &i, &i),
            Change::Rebuild
        );
        assert_eq!(
            classify(&a, &key(1, vec![[1; 12]], true), &i, &i),
            Change::Rebuild
        );
        assert_eq!(
            classify(&a, &key(1, vec![[0; 12]], false), &i, &t),
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
