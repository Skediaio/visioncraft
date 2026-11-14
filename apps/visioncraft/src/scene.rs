//! Turns the core `Project` plus loaded geometry into renderer meshes, materials and items, and
//! updates only what changed. The only place that builds `visioncraft_render::Mesh`/`Material`.

use glam::{Mat4, Vec3};
use std::collections::HashMap;
use std::sync::Arc;
use visioncraft_core::{
    Asset, Evaluation, Generator, ImportedModel, Material, ModelRef, Project, Shape, SurfaceKind,
    Terrain, area_lifts, areas_over, blocked_by_buildings, evaluate_in_scene,
    evaluate_surface_fill, ground_height, inside_outline, lawn_scatter, placed_shape, raycast,
    resolve_materials, shape_mesh, shape_mesh_on_terrain, water_areas,
};
use visioncraft_flora as flora;
use visioncraft_render::{ImpostorId, ItemId, LineVertex, Renderer};
pub use visioncraft_render::{MaterialId, MeshId};

use crate::library::{self, Library};
use crate::material_library;
use crate::placeholder::{self, MeshData};
use crate::plant_scene::{self, DirtyLog, EVERYTHING, Inst, Rect};
use crate::plants::{self, Flora, LAWN_SPECIES};

/// A loaded or generated design, with a counter that changes when its triangles do.
#[derive(Clone)]
pub struct Loaded {
    pub model: Arc<ImportedModel>,
    pub version: u64,
}

pub type LookKey = [u32; 13];

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
    /// Index of the material's maps in the library (0: none).
    pub tex: u32,
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
            tex: 0,
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
            f32::from_bits(self.tex),
        ]
        .map(f32::to_bits)
    }
}

const CLAY: Look = Look::plain(0.85, 0.85, 0.85, 0.9);
const UNASSIGNED: Look = Look::plain(0.6, 0.6, 0.6, 0.8);

/// The variation a material shows: its own, else (an untouched palette material) its library
/// look's.
pub fn shown_variation(lib: &Library, m: &Material) -> f32 {
    if m.variation > 0.0 || !library::spec_of(&m.maps).is_empty() {
        return m.variation;
    }
    material_library::overlay(lib, m).map_or(0.0, |o| o.variation)
}

fn look_of(m: &Material, lib: &mut Library) -> Look {
    let own = library::spec_of(&m.maps);
    // An unchanged palette material shows its library look while the library is there.
    let ov = if own.is_empty() {
        material_library::overlay(lib, m)
    } else {
        None
    };
    let spec = ov.as_ref().map_or(own, |o| o.spec.clone());
    let variation = if m.variation > 0.0 {
        m.variation
    } else {
        ov.as_ref().map_or(0.0, |o| o.variation)
    };
    let mut plain = Look {
        color: m.base_color,
        metallic: m.metallic,
        roughness: m.roughness,
        variation,
        tex: if spec.is_empty() {
            0
        } else {
            lib.intern(spec) as u32
        },
        ..Look::plain(0.0, 0.0, 0.0, 0.0)
    };
    if ov.is_some() {
        // the pictures decide
        plain.color = [1.0; 4];
        plain.metallic = 1.0;
        plain.roughness = 1.0;
    }
    match &m.kind {
        SurfaceKind::ArchitecturalGlass { tint, roughness } => Look {
            color: [tint[0], tint[1], tint[2], 0.06],
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
            relief: if ov.is_some() { 1.0 } else { *relief },
            gloss: *gloss,
            weathering: *weathering,
            scale_m: ov.as_ref().map_or(*map_scale_m, |o| o.scale_m),
            ..plain
        },
        // The tufts are scattered separately (`Scene::sync_lawns`); the ground under them is this.
        SurfaceKind::Lawn3d(_) => Look {
            variation: plain.variation.max(0.6),
            scale_m: ov.as_ref().map_or(1.0, |o| o.scale_m),
            relief: if ov.is_some() { 1.0 } else { 0.0 },
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
        Option<visioncraft_render::TextureId>,
        visioncraft_render::TextureId,
        visioncraft_render::TextureId,
    ),
    tint: [f32; 3],
) -> MaterialId {
    r.add_material(&plant_material_desc(kind, tex, tint))
}

/// The renderer material of a plant part (`plant_material`; also what a regenerated atlas
/// updates in place).
pub fn plant_material_desc(
    kind: flora::Material,
    tex: (
        visioncraft_render::TextureId,
        Option<visioncraft_render::TextureId>,
        visioncraft_render::TextureId,
        visioncraft_render::TextureId,
    ),
    tint: [f32; 3],
) -> visioncraft_render::Material {
    use visioncraft_render::{AlphaMode, Material as M};
    let (atlas, atlas_normal, bark, bark_normal) = tex;
    let leaf = |color: [f32; 4], foliage: bool| M {
        base_color: color,
        base_color_map: Some(atlas),
        normal_map: atlas_normal,
        roughness: 0.6,
        alpha_mode: AlphaMode::Cutout { threshold: 0.5 },
        double_sided: true,
        foliage,
        ..Default::default()
    };
    match kind {
        flora::Material::Bark => M {
            base_color_map: Some(bark),
            normal_map: Some(bark_normal),
            roughness: 0.9,
            ..Default::default()
        },
        flora::Material::Foliage => leaf([tint[0], tint[1], tint[2], 1.0], true),
        flora::Material::Flower => leaf([1.0; 4], true),
        flora::Material::Twigs => leaf([1.0; 4], false),
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
    /// The shape as meshed (an area's lift already in its elevation) and, for an area lying on
    /// the terrain, which terrain it was draped over.
    Shape(Box<Shape>, usize),
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

/// The 3D grass of one owner's lawn faces.
type LawnKey = (Geom, Mat4, Vec<(usize, [u32; 5], u64)>, u64, u8, Vec<u32>);

struct LawnGpu {
    key: LawnKey,
    items: Vec<ItemId>,
}

/// Tufts stop being drawn beyond this distance: far lawns are just the ground texture. The last
/// `LAWN_FADE_BAND_M` dither out (`set_item_cull_fade`), so the edge is a gradient, not a line.
const LAWN_FADE_M: f32 = 40.0;
const LAWN_FADE_BAND_M: f32 = 15.0;
/// Shrubs, ground plants and small objects fade out over this share of their cull distance.
const CULL_FADE_SHARE: f32 = 0.25;
/// At most this many tufts per square metre are drawn, and this many per lawn.
const LAWN_MAX_DENSITY: f32 = 300.0;
const LAWN_MAX_TUFTS: usize = 60_000;

/// Catalogue objects smaller than this (largest side, metres) are culled far away, at
/// `OBJECT_CULL_PER_M` metres per metre of size.
const SMALL_OBJECT_M: f32 = 4.0;
const OBJECT_CULL_PER_M: f32 = 500.0;

/// The largest LOD distance below `last` (0 when there is none).
fn parts_before(parts: &[(MeshId, MaterialId, f32)], last: f32) -> f32 {
    parts
        .iter()
        .map(|p| p.2)
        .filter(|d| *d < last)
        .fold(0.0, f32::max)
}

/// The instances of one (asset, variant) of a generator: its renderer items (a plant or a
/// catalogue model is one LOD item, a stand-in several), the LOD parts they draw (`None`: a
/// stand-in) and their matrices.
struct Group {
    items: Vec<ItemId>,
    parts: Option<LodParts>,
    /// The impostor the items were made with: one baked later (a shrub's keeps its mesh
    /// levels) still means new items.
    impostor: Option<ImpostorId>,
    matrices: Vec<Mat4>,
}

type LodParts = Vec<(MeshId, MaterialId, f32)>;

struct GenGpu {
    /// What the evaluation depended on besides the ground: the generator, the month, visibility.
    sig: (Generator, u8, bool),
    /// The dirty-log stamp it was evaluated at, and where it puts plants.
    stamp: u64,
    bounds: Option<Rect>,
    flora_rev: u64,
    library_rev: u64,
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
    /// Height and crown radius of the project's own species, for the building rule.
    custom_sizes: HashMap<String, (f32, f32)>,
    lawns: HashMap<String, LawnGpu>,
    materials: HashMap<LookKey, MaterialId>,
    /// Looks made for placeholders and the figure: never pruned.
    pinned: std::collections::HashSet<LookKey>,
    protos: HashMap<String, Vec<(MeshId, MaterialId)>>,
    /// Where the ground changed since which stamp: generators depend on it.
    dirty: DirtyLog,
    terrain: Option<Arc<Terrain>>,
    terrain_gpu: crate::terrain_gpu::TerrainGpu,
    pub timings: Timings,
    pub flora: Flora,
    /// The asset library: models, textures and lamps streamed in.
    pub library: Library,
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
        if let Some(id) = self.materials.get(&look.key()) {
            return *id;
        }
        let m = gpu_material(look);
        let id = r.add_material(&m);
        if look.tex != 0 {
            self.library.register_textured(id, m, look.tex as usize);
        }
        self.materials.insert(look.key(), id);
        id
    }

    /// Frees the materials of looks no owner shows any more (a colour dragged through many
    /// values makes one per value), once there are more than a few spare.
    fn prune_materials(&mut self, r: &mut Renderer) {
        const SPARE: usize = 32;
        let used: std::collections::HashSet<LookKey> = self
            .owners
            .values()
            .flat_map(|o| o.key.looks.iter().copied())
            .chain(self.pinned.iter().copied())
            .collect();
        if self.materials.len() <= used.len() + SPARE {
            return;
        }
        let unused: Vec<LookKey> = self
            .materials
            .keys()
            .filter(|k| !used.contains(*k))
            .copied()
            .collect();
        for k in unused {
            if let Some(id) = self.materials.remove(&k) {
                self.library.forget_textured(id);
                r.remove_material(id);
            }
        }
    }

    fn drop_owner(r: &mut Renderer, o: Owner) {
        for p in o.pieces {
            r.remove_item(p.item);
            r.remove_mesh(p.mesh);
        }
    }

    pub fn clear(&mut self, r: &mut Renderer) {
        for id in self.gens.keys() {
            self.library.forget_generator(id);
        }
        for (_, o) in self.owners.drain() {
            Self::drop_owner(r, o);
        }
        for (_, g) in self.gens.drain() {
            Self::drop_groups(r, g.groups);
        }
        for (_, l) in self.lawns.drain() {
            l.items.into_iter().for_each(|i| r.remove_item(i));
        }
        self.terrain_gpu.clear(r);
        self.terrain = None;
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
        // The terrain: chunks remade where it changed, and generators there re-evaluated.
        let old = self.terrain.clone();
        self.pinned.insert(CLAY.key());
        let any = self.material(r, &CLAY); // terrain items ignore their material
        if let Some(d) = self.terrain_gpu.sync(
            r,
            project.terrain.as_ref(),
            project.view.ground.map(|g| g.height),
            any,
        ) {
            match (&old, &project.terrain) {
                (Some(a), Some(b))
                    if a.origin == b.origin
                        && a.cell_m == b.cell_m
                        && (a.width, a.depth) == (b.width, b.depth) =>
                {
                    let at = |x: u32, z: u32| {
                        [
                            b.origin[0] + x as f32 * b.cell_m,
                            b.origin[1] + z as f32 * b.cell_m,
                        ]
                    };
                    self.dirty.mark([at(d.x0, d.z0), at(d.x1, d.z1)]);
                }
                (a, b) => {
                    let extents = [a, b].map(|t| t.as_ref().map(|t| t.extent()));
                    self.ground_changed(extents);
                }
            }
        }
        self.terrain.clone_from(&project.terrain);
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
        // How shapes sit on the ground: `visioncraft_core::shape`'s module docs.
        let lifts: HashMap<String, f32> = area_lifts(&project.shapes).into_iter().collect();
        let terrain = project.terrain.clone();
        // Water areas: the areas under them are cut out there (the pond's bed is the terrain's).
        let water = water_areas(project);
        // shortcut: the terrain's address tells versions apart; an edit makes a new Arc
        let terrain_key = terrain.as_ref().map_or(0, |t| Arc::as_ptr(t) as usize);
        for s in &project.shapes {
            let id = s.id().to_string();
            let (placed, lift, holes) = placed_shape(project, s, &lifts, &water);
            let xf = Mat4::from_translation(Vec3::new(0.0, lift, 0.0));
            let draped = matches!(&placed, Shape::Area(a) if a.drape && terrain.is_some());
            let holes: Vec<&[[f32; 2]]> = holes.iter().map(Vec::as_slice).collect();
            // What the mesh depends on besides the shape: the terrain and the water on top.
            let depends = if draped {
                holes
                    .iter()
                    .flat_map(|h| h.iter())
                    .fold(terrain_key, |k, p| {
                        k.rotate_left(7)
                            ^ (p[0].to_bits() as usize)
                            ^ ((p[1].to_bits() as usize) << 1)
                    })
            } else {
                0
            };
            let geom = Geom::Shape(Box::new(placed.clone()), depends);
            let visible = project.layer_visible(s.layer());
            // The mesh is only rebuilt when the shape (or the terrain it lies on) changed.
            let model = match self.owners.get(&id) {
                Some(o) if o.key.geom == geom => o.geom.clone(),
                _ => match match terrain.as_deref() {
                    // Cut along the terrain's own triangles: no hill edge pokes through.
                    Some(t) => shape_mesh_on_terrain(&placed, t, &holes),
                    None => shape_mesh(&placed),
                } {
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
            wanted.push((id, geom, model, xf, visible, true, is_area));
        }

        for (id, geom, model, xf, visible, is_shape, is_area) in wanted {
            live.push(id.clone());
            let mut owner_looks = |project: &Project| -> Vec<Look> {
                let resolved = resolve_materials(project, &id, &model);
                model
                    .faces
                    .iter()
                    .zip(resolved)
                    .map(|(f, m)| {
                        if clay {
                            CLAY
                        } else if let Some(i) = m {
                            look_of(&project.materials[i], &mut self.library)
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
        self.sync_lawns(r, project);
        self.prune_materials(r);
        problems
    }

    /// 3D grass on every face whose material is a lawn: tufts of `flora:lawn-tuft` scattered with
    /// the core's `lawn_scatter`, an LOD item that stops drawing at `LAWN_FADE_M`.
    fn sync_lawns(&mut self, r: &mut Renderer, project: &Project) {
        let month = project.time.month as u8;
        let mut live: Vec<String> = Vec::new();
        let Some(res) = plants::resolve(project, LAWN_SPECIES) else {
            return;
        };
        let class = plants::class(&res.params, month);
        let mut owners: Vec<&String> = self.owners.keys().collect();
        owners.sort();
        type Faces = Vec<(usize, visioncraft_core::material::Lawn3d, [f32; 3])>;
        let mut todo: Vec<(String, Faces)> = Vec::new();
        for id in owners {
            let o = &self.owners[id];
            if !o.key.visible || !o.is_shape {
                continue;
            }
            let faces: Faces = resolve_materials(project, id, &o.geom)
                .into_iter()
                .enumerate()
                .filter_map(|(f, m)| {
                    let m = project.materials.get(m?)?;
                    match &m.kind {
                        SurfaceKind::Lawn3d(l) => {
                            Some((f, *l, [m.base_color[0], m.base_color[1], m.base_color[2]]))
                        }
                        _ => None,
                    }
                })
                .collect();
            if !faces.is_empty() {
                todo.push((id.clone(), faces));
            }
        }
        for (id, faces) in todo {
            live.push(id.clone());
            let o = &self.owners[&id];
            // No grass under a terrace, path or pond drawn on top of this lawn.
            let over = areas_over(&project.shapes, &id);
            let over_key: Vec<u32> = over
                .iter()
                .flat_map(|o| o.iter().flat_map(|p| [p[0].to_bits(), p[1].to_bits()]))
                .collect();
            let key = (
                o.key.geom.clone(),
                o.xf,
                faces
                    .iter()
                    .map(|(f, l, c)| {
                        (
                            *f,
                            [
                                l.density_per_m2.to_bits(),
                                l.blade_height_m.to_bits(),
                                c[0].to_bits(),
                                c[1].to_bits(),
                                c[2].to_bits(),
                            ],
                            l.seed,
                        )
                    })
                    .collect::<Vec<_>>(),
                self.flora.rev,
                month,
                over_key,
            );
            if self.lawns.get(&id).is_some_and(|g| g.key == key) {
                continue;
            }
            if self.flora.parts(LAWN_SPECIES, class, 0).is_none() {
                self.flora.want(LAWN_SPECIES, class, month, &res.params);
                continue;
            }
            // One matrix list per variant, so the lawn does not repeat one tuft.
            let mut by_variant: Vec<Vec<Mat4>> = vec![Vec::new(); plants::VARIANTS as usize];
            let mut n = 0usize;
            // shortcut: one colour per lawn (its first lawn face's), fine until lawns mix colours
            let color = faces[0].2;
            for (face, lawn, _) in &faces {
                let lawn = visioncraft_core::material::Lawn3d {
                    density_per_m2: lawn.density_per_m2.min(LAWN_MAX_DENSITY),
                    ..*lawn
                };
                for t in lawn_scatter(&o.geom, *face, &lawn) {
                    if n >= LAWN_MAX_TUFTS {
                        break;
                    }
                    let p = o.xf.transform_point3(Vec3::from(t.position));
                    if over.iter().any(|a| inside_outline([p.x, p.z], a)) {
                        continue;
                    }
                    let s = (t.height_m / 0.08).clamp(0.3, 2.5);
                    by_variant[n % plants::VARIANTS as usize].push(
                        Mat4::from_translation(p)
                            * Mat4::from_rotation_y(t.rotation_y_deg.to_radians())
                            * Mat4::from_scale(Vec3::splat(s)),
                    );
                    n += 1;
                }
            }
            if let Some(old) = self.lawns.remove(&id) {
                old.items.into_iter().for_each(|i| r.remove_item(i));
            }
            let mut items = Vec::new();
            for (v, matrices) in by_variant.iter().enumerate() {
                let v = v as u32;
                if matrices.is_empty() {
                    continue;
                }
                let Some(parts) = self.flora.tinted(r, LAWN_SPECIES, class, v, color) else {
                    continue;
                };
                // The coarsest level is the last one drawn, fading out, then nothing.
                let parts: Vec<_> = parts
                    .iter()
                    .map(|&(m, mat, d)| (m, mat, d.min(LAWN_FADE_M)))
                    .collect();
                let item = r.add_lod_item(&parts, matrices);
                r.set_item_cull_fade(item, LAWN_FADE_BAND_M);
                items.push(item);
            }
            self.lawns.insert(id, LawnGpu { key, items });
        }
        let gone: Vec<String> = self
            .lawns
            .keys()
            .filter(|k| !live.contains(k))
            .cloned()
            .collect();
        for id in gone {
            if let Some(l) = self.lawns.remove(&id) {
                l.items.into_iter().for_each(|i| r.remove_item(i));
            }
        }
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
        self.custom_sizes = project
            .species
            .iter()
            .map(|c| {
                (
                    c.id.clone(),
                    plant_scene::plant_size(project, assets, &c.id),
                )
            })
            .collect();
        let ids: Vec<&str> = project.generators.iter().map(Generator::id).collect();
        let gone: Vec<String> = self
            .gens
            .keys()
            .filter(|k| !ids.contains(&k.as_str()))
            .cloned()
            .collect();
        for id in gone {
            self.library.forget_generator(&id);
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
                if o.flora_rev != self.flora.rev || o.library_rev != self.library.rev {
                    self.upgrade(r, project, assets, month, &mut o.groups);
                    o.flora_rev = self.flora.rev;
                    o.library_rev = self.library.rev;
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

    /// What a group draws as its real self: a catalogue asset's resident LOD parts (the
    /// catalogue wins over a plant of the same id), else the generated plant. `None` while only a
    /// stand-in can show.
    fn source(&self, project: &Project, asset: &str, variant: u32, month: u8) -> Option<LodParts> {
        if self.library.knows(asset) {
            let mut parts = self.library.parts(asset)?;
            // Small objects (a pot, a bench) stop drawing far away, at a size-proportional distance.
            let size = self
                .library
                .catalog
                .as_ref()
                .and_then(|c| c.asset(asset))
                .map_or(1.0, |a| a.size_m.into_iter().fold(0.0, f32::max));
            if size < SMALL_OBJECT_M {
                let cull = (OBJECT_CULL_PER_M * size).clamp(60.0, 1500.0);
                let last = parts.iter().map(|p| p.2).fold(0.0, f32::max);
                let cull = cull.max(parts_before(&parts, last) + 1.0);
                for p in parts.iter_mut().filter(|p| p.2 == last) {
                    p.2 = p.2.min(cull);
                }
            }
            return Some(parts);
        }
        let res = plants::resolve(project, asset)?;
        let class = plants::class(&res.params, month);
        self.flora.parts(asset, class, variant).map(<[_]>::to_vec)
    }

    /// Renderer items for one group: one LOD item for the real thing (a tree's impostor after its
    /// meshes), else stand-in items (and the plant is asked for).
    #[allow(clippy::too_many_arguments)]
    fn make_items(
        &mut self,
        r: &mut Renderer,
        project: &Project,
        assets: &[Asset],
        asset: &str,
        variant: u32,
        month: u8,
        src: Option<&LodParts>,
        matrices: &[Mat4],
    ) -> Vec<ItemId> {
        if let Some(parts) = src {
            let item = r.add_lod_item(parts, matrices);
            // A hedge's shadow is a band, not one blob per segment.
            let clipped = asset
                .strip_prefix("flora:")
                .and_then(flora::Species::by_id)
                .is_some_and(|s| matches!(s.params.form, flora::Form::Clipped(_)));
            r.set_item_shadow_exact(item, clipped);
            let impostor = self.impostor_for(project, asset, variant, month);
            let last = parts.iter().map(|p| p.2).fold(0.0, f32::max);
            match impostor {
                // fails only for a removed impostor: the tree then stops at its far mesh
                Some((imp, max, small)) => {
                    if r.set_item_impostor(item, imp, max).is_ok() {
                        // At eye level a shrub far away is hidden anyway: its reach follows the camera.
                        r.set_item_follows_reach(item, small);
                    }
                }
                // Culled at a finite distance (shrubs, small objects): dither out before it.
                None if last.is_finite() => r.set_item_cull_fade(item, last * CULL_FADE_SHARE),
                None => {}
            }
            return vec![item];
        }
        let plant = (!self.library.knows(asset))
            .then(|| plants::resolve(project, asset))
            .flatten();
        let protos = match plant {
            Some(res) => {
                let class = plants::class(&res.params, month);
                self.flora.want(asset, class, month, &res.params);
                let stand_in = placeholder::flora_asset(asset, &res);
                self.proto(r, asset, Some(&stand_in))
            }
            // A catalogue model still loading, or a plain asset: its box from the project assets.
            None => self.proto(r, asset, assets.iter().find(|a| a.id == asset)),
        };
        protos
            .into_iter()
            .map(|(mesh, mat)| r.add_item(mesh, mat, matrices))
            .collect()
    }

    /// A plant's baked impostor and how far it draws (none for catalogue models).
    fn impostor_for(
        &self,
        project: &Project,
        asset: &str,
        variant: u32,
        month: u8,
    ) -> Option<(ImpostorId, f32, bool)> {
        if self.library.knows(asset) {
            return None;
        }
        let res = plants::resolve(project, asset)?;
        let class = plants::class(&res.params, month);
        self.flora.impostor(asset, class, variant)
    }

    /// Swaps stand-ins for the real thing once a plant generation or a catalogue model finished,
    /// and follows the catalogue's resident levels (a finer one arrived, one was evicted).
    fn upgrade(
        &mut self,
        r: &mut Renderer,
        project: &Project,
        assets: &[Asset],
        month: u8,
        groups: &mut HashMap<(String, u32), Group>,
    ) {
        for ((asset, variant), grp) in groups.iter_mut() {
            let src = self.source(project, asset, *variant, month);
            let imp = self
                .impostor_for(project, asset, *variant, month)
                .map(|i| i.0);
            if src == grp.parts && imp == grp.impostor {
                continue;
            }
            grp.items.drain(..).for_each(|i| r.remove_item(i));
            grp.items = self.make_items(
                r,
                project,
                assets,
                asset,
                *variant,
                month,
                src.as_ref(),
                &grp.matrices,
            );
            grp.parts = src;
            grp.impostor = imp;
        }
    }

    /// Evaluates one generator and brings its renderer items in line: instance lists per
    /// (asset, variant) are updated in place, items are only created or removed for groups that
    /// appear, vanish or change what they draw.
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
        self.library.forget_generator(g.id());
        for (asset, variant, matrices) in
            plant_scene::group_placements(&placements, plants::VARIANTS)
        {
            if self.library.knows(&asset) {
                // the library streams the level the nearest instance needs
                self.library.note(g.id(), &asset, &matrices);
            }
            let src = self.source(project, &asset, variant, month);
            let imp = self
                .impostor_for(project, &asset, variant, month)
                .map(|i| i.0);
            let items = match old_groups.remove(&(asset.clone(), variant)) {
                Some(prev) if prev.parts == src && prev.impostor == imp => {
                    prev.items
                        .iter()
                        .for_each(|i| r.set_instances(*i, &matrices));
                    prev.items
                }
                prev => {
                    if let Some(p) = prev {
                        p.items.into_iter().for_each(|i| r.remove_item(i));
                    }
                    self.make_items(
                        r,
                        project,
                        assets,
                        &asset,
                        variant,
                        month,
                        src.as_ref(),
                        &matrices,
                    )
                }
            };
            groups.insert(
                (asset, variant),
                Group {
                    items,
                    parts: src,
                    impostor: imp,
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
            library_rev: self.library.rev,
            groups,
            insts,
            truncated,
        }
    }

    /// The terrain's painted layers from their materials (pictures arrive over time).
    pub fn sync_terrain_layers(&mut self, r: &mut Renderer, project: &Project) {
        self.terrain_gpu
            .sync_layers(r, project, &mut self.library, &mut |m, lib| look_of(m, lib));
        // The endless ground wears the terrain's base layer, so the terrain's edge cannot be
        // seen against it (one ground look, in the overview and in the plan).
        // Without a terrain it wears the lawn: a real ground material (its texture's mips give
        // the far colour, the macro variation keeps it from tiling), not a flat colour.
        let base_id = project
            .terrain
            .as_ref()
            .and_then(|t| t.layers.first())
            .map_or("lawn", |l| l.material.as_str());
        let base = project.materials.iter().find(|m| m.id == base_id);
        let ground = base.map(|m| {
            let look = look_of(m, &mut self.library);
            self.material(r, &look)
        });
        r.set_ground_material(ground);
    }

    /// One step of lazy plant generation; true when a generation finished (the scene must sync again).
    /// `px_per_m(d)`: screen pixels a metre spans at distance `d` (the camera's projection).
    pub fn pump(&mut self, r: &mut Renderer, eye: Vec3, px_per_m: &dyn Fn(f32) -> f32) -> bool {
        if self.library.store.tick().is_multiple_of(10) {
            self.library.set_need(self.texel_need(eye, px_per_m));
        }
        let library = self.library.pump(r, eye);
        self.flora.pump(r) | library
    }

    /// Screen pixels per texture repeat of each textured material at its nearest visible
    /// surface: what decides whether 4096 texels per repeat show more than 2048 would.
    fn texel_need(&self, eye: Vec3, px_per_m: &dyn Fn(f32) -> f32) -> HashMap<usize, f32> {
        let mut need: HashMap<usize, f32> = HashMap::new();
        for o in self.owners.values().filter(|o| o.key.visible) {
            let Some((lo, hi)) = world_bounds(&o.geom, &o.xf) else {
                continue;
            };
            let d = (eye.clamp(lo, hi) - eye).length().max(0.1);
            for k in &o.key.looks {
                let (scale, tex) = (f32::from_bits(k[11]), k[12] as usize);
                if tex != 0 {
                    let n = scale * px_per_m(d);
                    let e = need.entry(tex).or_insert(0.0);
                    *e = e.max(n);
                }
            }
        }
        need
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
            _ => evaluate_in_scene(
                g,
                &self.ground_fn(),
                &|x, z| self.open_ground(x, z),
                &|p| self.up_to_building(p),
                &|a| self.plant_dims(a),
            ),
        }
    }

    /// Height and crown radius of an asset at scale 1.
    pub fn plant_dims(&self, asset: &str) -> (f32, f32) {
        if let Some(s) = asset.strip_prefix("flora:").and_then(flora::Species::by_id) {
            return (s.mature_height_m, s.mature_width_m / 2.0);
        }
        if let Some(d) = self.custom_sizes.get(asset) {
            return *d;
        }
        self.library
            .catalog
            .as_ref()
            .and_then(|c| c.asset(asset))
            .map_or((1.0, 0.4), |a| {
                (a.size_m[1], a.size_m[0].max(a.size_m[2]) / 2.0)
            })
    }

    /// The building rule (`visioncraft_core::blocked_by_buildings`) against this scene: the
    /// open ground is the terrain and the areas, the buildings are models and blocks.
    pub fn in_building(&self, at: Vec3, height: f32, radius: f32) -> bool {
        let open = self.open_ground(at.x, at.z).unwrap_or(0.0);
        blocked_by_buildings(at.to_array(), height, radius, open, &|p| {
            self.up_to_building(p)
        })
    }

    /// Only the overhead part of the rule, for a plant placed by hand where the pointer is: a
    /// pot on a roof is fine, a tree under a canopy lower than itself is not.
    pub fn under_building(&self, at: Vec3, height: f32, radius: f32) -> bool {
        blocked_by_buildings(at.to_array(), height, radius, at.y, &|p| {
            self.up_to_building(p)
        })
    }

    /// The terrain and areas at `(x, z)`, models and blocks left out.
    pub fn open_ground(&self, x: f32, z: f32) -> Option<f32> {
        let terrain = self.terrain.as_ref().and_then(|t| t.height_at(x, z));
        self.owners
            .values()
            .filter(|o| o.is_area && o.key.visible)
            .filter_map(|o| {
                let inv = o.xf.inverse();
                let local = inv.transform_point3(Vec3::new(x, 0.0, z));
                ground_height(&o.geom, local.x, local.z)
                    .map(|y| o.xf.transform_point3(Vec3::new(local.x, y, local.z)).y)
            })
            .fold(terrain, |a: Option<f32>, y| Some(a.map_or(y, |a| a.max(y))))
    }

    /// Distance straight up from `p` to the nearest model or block, if any.
    fn up_to_building(&self, p: [f32; 3]) -> Option<f32> {
        self.owners
            .values()
            .filter(|o| !o.is_area && o.key.visible)
            .filter_map(|o| {
                let inv = o.xf.inverse();
                let lo = inv.transform_point3(Vec3::from(p));
                let ld = inv.transform_vector3(Vec3::Y);
                let h = raycast::raycast(&o.geom, lo.to_array(), ld.to_array())?;
                let hit = o.xf.transform_point3(Vec3::from(h.point));
                Some(hit.y - p[1])
            })
            .reduce(f32::min)
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
                self.pinned.insert(look.key());
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
                    let look = Look::plain(0.05, 0.05, 0.06, 0.9);
                    self.pinned.insert(look.key());
                    let mat = self.material(r, &look);
                    self.people = Some(r.add_item(mesh, mat, at));
                }
            }
        }
    }

    // ---- queries ----

    /// Whether a person could stand at `(x, z)`: not inside a plant's crown or a hedge (its
    /// spread, from the species' width or the object's size), not under or inside a block or
    /// model (anything more than 30 cm above the ground there), not in water.
    pub fn free_to_stand(&self, project: &Project, x: f32, z: f32) -> bool {
        let ground = self.floor_below(x, 500.0, z).unwrap_or(0.0);
        let terrain = self
            .terrain
            .as_ref()
            .and_then(|t| t.height_at(x, z))
            .unwrap_or(ground);
        if self
            .raycast(Vec3::new(x, terrain + 200.0, z), Vec3::NEG_Y, false)
            .is_some_and(|p| p.point.y > terrain + 0.3)
        {
            return false;
        }
        let water = project.shapes.iter().any(|s| {
            let Shape::Area(a) = s else { return false };
            inside_outline([x, z], &a.outline)
                && self.owners.get(&a.id).is_some_and(|o| {
                    resolve_materials(project, &a.id, &o.geom)
                        .into_iter()
                        .any(|m| {
                            m.and_then(|i| project.materials.get(i)).is_some_and(|m| {
                                matches!(m.kind, SurfaceKind::AnimatedWater { .. })
                            })
                        })
                })
        });
        if water {
            return false;
        }
        let half_width = |asset: &str| -> f32 {
            asset
                .strip_prefix("flora:")
                .and_then(flora::Species::by_id)
                .map(|s| s.mature_width_m / 2.0)
                .or_else(|| {
                    self.library
                        .catalog
                        .as_ref()
                        .and_then(|c| c.asset(asset))
                        .map(|a| a.size_m[0].max(a.size_m[2]) / 2.0)
                })
                .unwrap_or(0.5)
        };
        !self.gens.values().any(|g| {
            g.groups.iter().any(|((asset, _), grp)| {
                let r = half_width(asset);
                grp.matrices.iter().any(|m| {
                    let scale = m.x_axis.truncate().length();
                    let at = m.w_axis;
                    (at.x - x).hypot(at.z - z) < r * scale + 0.3
                })
            })
        })
    }

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

    /// The core documents `rotation_y_deg` as counter-clockwise seen from above. With north
    /// (-Z) up and east (+X) right in the plan view, a quarter turn takes an object's east side
    /// to the north, and a fixture's light (the core's `LightSpec::light`) turns with it.
    #[test]
    fn rotation_is_counter_clockwise_seen_from_above_for_models_and_their_lights() {
        let m: ModelRef = serde_json::from_value(serde_json::json!({"id": "m1", "name": "x",
            "kind": "obj", "source": "x.obj", "transform": {"rotation_y_deg": 90.0}}))
        .unwrap();
        let east = model_matrix(&m).transform_vector3(Vec3::X);
        assert!((east - Vec3::NEG_Z).length() < 1e-5, "{east}");
        let spec = visioncraft_core::LightSpec {
            kind: visioncraft_core::LightKind::Spot {
                direction: [1.0, -1.0, 0.0],
                inner_cone_deg: 10.0,
                outer_cone_deg: 20.0,
            },
            color_temperature_k: 3000.0,
            lumens: 100.0,
            range_m: 5.0,
            offset_m: [1.0, 2.0, 0.0],
        };
        let l = spec.light(Default::default(), [0.0; 3], 90.0, 1.0);
        let at = Vec3::from(l.position);
        assert!((at - model_matrix(&m).transform_point3(Vec3::new(1.0, 2.0, 0.0))).length() < 1e-5);
        let visioncraft_core::LightKind::Spot { direction, .. } = l.kind else {
            panic!("a spot")
        };
        assert!((Vec3::from(direction) - Vec3::new(0.0, -1.0, -1.0)).length() < 1e-5);
        // the plan view shows north up: the turned light sits above the model's origin on screen
        let mut c = crate::camera::Camera::default();
        c.view = crate::camera::View::Top;
        let (v, p, _) = c.matrices(1.0);
        assert!((p * v).project_point3(at).y > (p * v).project_point3(Vec3::ZERO).y);
    }

    fn key(version: u64, looks: Vec<LookKey>, visible: bool) -> OwnerKey {
        OwnerKey {
            geom: Geom::Model(version),
            visible,
            looks,
        }
    }

    #[test]
    fn classify_picks_the_cheapest_update() {
        let a = key(1, vec![[0; 13]], true);
        let (i, t) = (Mat4::IDENTITY, Mat4::from_translation(Vec3::X));
        assert_eq!(classify(&a, &a.clone(), &i, &i), Change::Keep);
        assert_eq!(classify(&a, &a.clone(), &i, &t), Change::Transform);
        assert_eq!(
            classify(&a, &key(2, vec![[0; 13]], true), &i, &i),
            Change::Rebuild
        );
        assert_eq!(
            classify(&a, &key(1, vec![[1; 13]], true), &i, &i),
            Change::Rebuild
        );
        assert_eq!(
            classify(&a, &key(1, vec![[0; 13]], false), &i, &t),
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
