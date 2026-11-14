//! Turns the core `Project` plus loaded geometry into renderer meshes, materials and items, and
//! updates only what changed. The only place that builds `visioncraft_render::Mesh`/`Material`.

use glam::{Mat4, Vec3};
use std::collections::HashMap;
use std::sync::Arc;
use visioncraft_core::{
    AREA_LIFT_M, Asset, Evaluation, Generator, ImportedModel, LawnWindow, Material, ModelRef,
    Project, Shape, SurfaceKind, Terrain, area_lifts, areas_over, blocked_by_buildings,
    evaluate_in_scene, evaluate_surface_fill, ground_height, inside_outline, lawn_area,
    lawn_scatter_within, placed_shape, raycast, resolve_materials, shape_mesh,
    shape_mesh_on_terrain, water_areas,
};
use visioncraft_flora as flora;
use visioncraft_render::{ImpostorId, ItemId, LineVertex, Renderer};
pub use visioncraft_render::{MaterialId, MeshId};

use crate::library::{self, Library};
use crate::material_library;
use crate::placeholder::{self, MeshData};
use crate::plant_scene::{self, DirtyLog, EVERYTHING, Inst, Rect};
use crate::plants::{self, Flora, LAWN_SPECIES};

mod clutter;

/// A loaded or generated design, with a counter that changes when its triangles do.
#[derive(Clone)]
pub struct Loaded {
    pub model: Arc<ImportedModel>,
    pub version: u64,
}

pub type LookKey = [u32; 14];

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
    /// Mowing stripes, packed as the renderer takes them (0: none).
    pub stripes: u32,
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
            stripes: 0,
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
            f32::from_bits(self.stripes),
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

fn look_of(m: &Material, lib: &mut Library, month: u32) -> Look {
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
    if let Some(o) = &ov {
        // the pictures decide, under the overlay's tint
        plain.color = [o.tint[0], o.tint[1], o.tint[2], 1.0];
        plain.metallic = 1.0;
        plain.roughness = 1.0;
    }
    match &m.kind {
        SurfaceKind::ArchitecturalGlass { tint, roughness } => Look {
            color: [tint[0], tint[1], tint[2], 0.02],
            roughness: *roughness,
            surface: 1,
            ..plain
        },
        // The light the water body scatters back is a few per cent at most (a sunlit pond is
        // dark green, not teal): the reflection of the sky and garden carries the surface, the
        // bed shows looking down near the edge.
        SurfaceKind::AnimatedWater { tint, .. } => Look {
            color: [tint[0] * 0.08, tint[1] * 0.08, tint[2] * 0.08, 0.3],
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
        SurfaceKind::Lawn3d(l) => {
            let t = lawn_season(month);
            let c = plain.color;
            Look {
                color: [c[0] * t[0], c[1] * t[1], c[2] * t[2], c[3]],
                variation: plain.variation.max(0.6),
                scale_m: ov.as_ref().map_or(1.0, |o| o.scale_m),
                relief: if ov.is_some() { 1.0 } else { 0.0 },
                stripes: stripes_packed(l),
                ..plain
            }
        }
    }
}

/// A lawn's mowing stripes as the renderer takes them: width in cm, direction in degrees << 16.
fn stripes_packed(l: &visioncraft_core::material::Lawn3d) -> u32 {
    if l.stripes_m <= 0.0 {
        return 0;
    }
    let cm = (l.stripes_m * 100.0).round().clamp(1.0, 65535.0) as u32;
    let deg = l.stripes_deg.rem_euclid(360.0).round() as u32 % 360;
    cm | deg << 16
}

/// What the season does to a lawn's colour: duller and a little straw-coloured in winter,
/// between in the months around it, as it is all summer.
fn lawn_season(month: u32) -> [f32; 3] {
    match month {
        12 | 1 | 2 => [0.78, 0.7, 0.52],
        3 | 11 => [0.9, 0.85, 0.76],
        _ => [1.0; 3],
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
        stripes: look.stripes,
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

/// A plant mesh part (positions, normals, UVs, wind) into the renderer; a `crown` (the foliage
/// of a plant) with the sky occlusion of its leaves (`crown_occlusion`).
pub fn upload_plant_mesh(r: &mut Renderer, p: &flora::MeshPart, crown: bool) -> Option<MeshId> {
    let occlusion = crown.then(|| crown_occlusion(&p.positions));
    let mesh = visioncraft_render::Mesh {
        positions: &p.positions,
        normals: &p.normals,
        indices: &p.indices,
        face_ids: occlusion.as_deref(),
        uvs: Some(&p.uvs),
        wind: Some(&p.wind),
    };
    r.add_mesh(&mesh).ok()
}

/// Tag of a face id that carries a leaf's sky occlusion in its low 16 bits (scene.wgsl's
/// `CROWN_TAG`); plain face ids are small numbers.
const CROWN_TAG: u32 = 0xC000_0000;

/// Per leaf vertex, how much sky it sees through the crown, as a tagged face id: leaves deep
/// under the crown's top and inside it see little (an ellipsoid round the leaves' bounds; light
/// falls through a crown in a few layers of leaves). The tops keep most of it, the bottom inside down
/// to about a third, so a crown seen from above is not one pale, evenly lit surface; the sun
/// on a leaf is dimmed by it too (scene.wgsl `light`).
pub fn crown_occlusion(positions: &[[f32; 3]]) -> Vec<u32> {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in positions {
        lo = lo.min(Vec3::from(*p));
        hi = hi.max(Vec3::from(*p));
    }
    let (c, rad) = ((lo + hi) * 0.5, ((hi - lo) * 0.5).max(Vec3::splat(1e-3)));
    positions
        .iter()
        .map(|p| {
            let q = (Vec3::from(*p) - c) / rad;
            let top = c.y + rad.y * (1.0 - q.x * q.x - q.z * q.z).max(0.0).sqrt();
            let below = ((top - p[1]) / (2.0 * rad.y)).clamp(0.0, 1.0);
            let inside = (1.0 - q.length()).clamp(0.0, 1.0);
            let s = below.min(0.75) / 0.75;
            // Even the top leaves have neighbours that hide part of the sky.
            let sky = (0.85 - 0.45 * s * s * (3.0 - 2.0 * s) - 0.2 * inside).max(0.3);
            CROWN_TAG | (sky * 65535.0) as u32
        })
        .collect()
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
type LawnKey = (
    Geom,
    Mat4,
    Vec<(usize, [u32; 5], u64)>,
    u64,
    u8,
    Vec<u32>,
    Option<[i32; 2]>,
);

struct LawnGpu {
    key: LawnKey,
    items: Vec<ItemId>,
}

/// A lawn's tufts being scattered: per variant, their matrices.
struct LawnJob {
    key: LawnKey,
    /// Scattered around the eye: drawn out to the window's edge, where its tufts have thinned
    /// out, rather than culled by distance cluster by cluster (`LAWN_FADE_M`).
    follow: bool,
    class: u32,
    color: [f32; 3],
    slot: plants::Slot<Vec<Vec<Mat4>>>,
    /// The scattered matrices once arrived, and the items drawn so far (one a frame).
    matrices: Option<Vec<Vec<Mat4>>>,
    built: Vec<Option<ItemId>>,
}

impl LawnJob {
    fn drop_items(self, r: &mut Renderer) {
        self.built
            .into_iter()
            .flatten()
            .for_each(|i| r.remove_item(i));
    }
}

/// Tufts stop being drawn beyond this distance: far lawns are just the ground texture. The last
/// `LAWN_FADE_BAND_M` dither out (`set_item_cull_fade`), so the edge is a gradient, not a line.
const LAWN_FADE_M: f32 = 40.0;
const LAWN_FADE_BAND_M: f32 = 15.0;
/// Shrubs, ground plants and small objects fade out over this share of their cull distance.
const CULL_FADE_SHARE: f32 = 0.25;
/// At most this many tufts per square metre are drawn, and this many per lawn; twice the
/// density and four times the count, with narrower blades, when dense (HIGH and ULTRA).
const LAWN_MAX_DENSITY: f32 = 300.0;
const LAWN_MAX_TUFTS: usize = 60_000;
/// A lawn over its tuft budget is scattered around the eye's cell of this size, out to
/// `LAWN_FADE_M` plus this from the cell's centre, and again when the eye moves to another cell.
const LAWN_EYE_CELL_M: f32 = 8.0;
/// Tufts farther than this from the window's centre are drawn in the far colour.
const LAWN_FAR_M: f32 = 16.0;
/// Snow deeper than this (`view.snow`) covers the lawn's blades.
const LAWN_SNOW_BURIES: f32 = 0.55;
/// Linear mean of the grass picture a plain lawn shows from the library (grass001), so the
/// tufts take the colour of the ground they stand in.
const LAWN_PICTURE_MEAN: [f32; 3] = [0.059, 0.107, 0.022];
/// Upright blades catch less of a high sun than the flat ground under them: lifted this much,
/// they read as the same lawn in sun and shade instead of dark strokes on it.
const LAWN_BLADE_LIFT: f32 = 1.6;

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
    /// The growth stage its plants are drawn at (`When::age`), and the one wanted (the same
    /// once generated).
    age: Option<u8>,
    want: Option<u8>,
}

/// What a plant generation depends on besides the species: the month (its season) and, while
/// the years after planting are shown, its growth stage in tenths of maturity (1..=9; none: full
/// grown).
#[derive(Clone, Copy, Debug, PartialEq)]
struct When {
    month: u8,
    age: Option<u8>,
}

impl When {
    /// The flora cache class and the parameters to generate with.
    fn class(&self, params: &flora::PlantParams) -> (u32, flora::PlantParams) {
        let mut p = params.clone();
        if let Some(a) = self.age {
            p.age = f32::from(a) / 10.0;
        }
        (
            plants::class(params, self.month) | (u32::from(self.age.unwrap_or(0)) << 8),
            p,
        )
    }
}

type LodParts = Vec<(MeshId, MaterialId, f32)>;

struct GenGpu {
    /// What the evaluation depended on besides the ground: the generator, the month, visibility,
    /// the years after planting shown.
    sig: (Generator, u8, bool, Option<u32>),
    /// The dirty-log stamp it was evaluated at, and where it puts plants.
    stamp: u64,
    bounds: Option<Rect>,
    flora_rev: u64,
    library_rev: u64,
    groups: HashMap<(String, u32), Group>,
    insts: Vec<Inst>,
    truncated: bool,
    empty: Option<&'static str>,
    /// Its plants, drawn or not (on a hidden layer).
    count: usize,
    hidden: bool,
}

/// Plants of one generator, as the panel shows them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GenStats {
    pub count: usize,
    pub truncated: bool,
    /// Why it placed nothing (`Scene::why_empty`), in plain words.
    pub empty: Option<&'static str>,
    /// On a hidden layer: counted, not drawn.
    pub hidden: bool,
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

/// The trees and conifers placed one by one, with their crown circle [x, z, radius] as grown
/// for the years shown.
fn tree_crowns(project: &Project) -> Vec<(&visioncraft_core::planting::Single, [f32; 3])> {
    project
        .generators
        .iter()
        .filter_map(|g| {
            let Generator::Single(s) = g else { return None };
            let gr = visioncraft_core::growth::growth_of(&s.asset, None);
            matches!(
                gr.grower,
                visioncraft_core::growth::Grower::Tree | visioncraft_core::growth::Grower::Conifer
            )
            .then(|| {
                let width = match project.view.years {
                    Some(y) => {
                        let planted = s.planted_age_years.unwrap_or(gr.planted_age_years);
                        gr.mature_width_m * gr.size(planted + y).width
                    }
                    None => gr.mature_width_m,
                };
                (s, [s.position[0], s.position[2], width * s.scale * 0.5])
            })
        })
        .collect()
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
    lawn_jobs: HashMap<String, LawnJob>,
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
    /// Denser, finer lawn tufts (HIGH and ULTRA; `set_dense_lawns`).
    dense_lawns: bool,
    /// The eye's cell (`LAWN_EYE_CELL_M` squares): lawns over their tuft budget are scattered
    /// around it (`lawns_follow`), and again when it moves to another cell.
    lawn_eye: [i32; 2],
    lawns_follow: bool,
    /// The tuft budget's multiple of the quality's (`set_lawn_budget`): more on fast GPUs.
    lawn_budget: f32,
    /// Fallen leaves, stones, twigs and lawn edges (`clutter`).
    clutter: clutter::Clutter,
    /// LOW quality: no clutter whatever the project says (`set_clutter_low`).
    clutter_low: bool,
    /// Each model's faces lying on the ground (`visioncraft_core::ground_faces`), with what
    /// they were found for: the model's version, its matrix and the terrain.
    model_ground: HashMap<String, ModelGround>,
    /// Those of every visible model: draped areas are cut out under them and grow no grass.
    ground_faces: Arc<Vec<[[f32; 2]; 3]>>,
}

type ModelGround = ((u64, [u32; 16], usize), Arc<Vec<[[f32; 2]; 3]>>);

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
        self.drop_lawns(r);
        self.clutter.clear(r);
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
        // shortcut: the terrain's address tells versions apart; an edit makes a new Arc
        let terrain_key = project
            .terrain
            .as_ref()
            .map_or(0, |t| Arc::as_ptr(t) as usize);
        let mut on_ground: Vec<[[f32; 2]; 3]> = Vec::new();
        for m in &project.models {
            if let Some(l) = loaded.get(&m.id) {
                let visible = project.layer_visible(&m.layer);
                let mx = model_matrix(m);
                // A model lying on the ground is drawn as an area is (`ground_faces`' rule).
                let key = (l.version, mx.to_cols_array().map(f32::to_bits), terrain_key);
                let faces = match self.model_ground.get(&m.id) {
                    Some((k, f)) if *k == key => f.clone(),
                    _ => {
                        let t = project.terrain.as_deref();
                        let f = Arc::new(visioncraft_core::ground_faces(
                            &l.model,
                            &|p| mx.transform_point3(Vec3::from(p)).into(),
                            &|x, z| t.and_then(|t| t.height_at(x, z)),
                        ));
                        self.model_ground.insert(m.id.clone(), (key, f.clone()));
                        f
                    }
                };
                let lift = if faces.is_empty() { 0.0 } else { AREA_LIFT_M };
                if visible {
                    on_ground.extend(faces.iter());
                }
                wanted.push((
                    m.id.clone(),
                    Geom::Model(l.version),
                    l.model.clone(),
                    Mat4::from_translation(Vec3::Y * lift) * mx,
                    visible,
                    false,
                    false,
                ));
            }
        }
        self.model_ground
            .retain(|id, _| project.models.iter().any(|m| &m.id == id));
        if *self.ground_faces != on_ground {
            self.ground_faces = Arc::new(on_ground);
        }
        // How shapes sit on the ground: `visioncraft_core::shape`'s module docs.
        let lifts: HashMap<String, f32> = area_lifts(&project.shapes).into_iter().collect();
        let terrain = project.terrain.clone();
        // Water areas: the areas under them are cut out there (the pond's bed is the terrain's).
        let water = water_areas(project);
        for s in &project.shapes {
            let id = s.id().to_string();
            let (placed, lift, holes) = placed_shape(project, s, &lifts, &water);
            let xf = Mat4::from_translation(Vec3::new(0.0, lift, 0.0));
            let draped = matches!(&placed, Shape::Area(a) if a.drape && terrain.is_some());
            let mut holes: Vec<&[[f32; 2]]> = holes.iter().map(Vec::as_slice).collect();
            if draped {
                holes.extend(self.ground_faces.iter().map(|f| f.as_slice()));
            }
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
                            look_of(
                                &project.materials[i],
                                &mut self.library,
                                u32::from(project.plant_month()),
                            )
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
        self.sync_clutter(r, project);
        self.prune_materials(r);
        problems
    }

    /// Draws the lawns whose tufts are scattered (`sync_lawns`); called every frame too. True
    /// while some are still being scattered.
    pub fn finish_lawns(&mut self, r: &mut Renderer) -> bool {
        // One variant's item per lawn and frame: a dense lawn's quarter of a million matrices
        // at once took 50 ms.
        let ids: Vec<String> = self.lawn_jobs.keys().cloned().collect();
        for id in ids {
            let Some(job) = self.lawn_jobs.get_mut(&id) else {
                continue;
            };
            if job.matrices.is_none() {
                job.matrices = job.slot.lock().ok().and_then(|mut s| s.take());
            }
            let Some(matrices) = job.matrices.as_ref() else {
                continue;
            };
            if let Some(v) = (job.built.len()..matrices.len()).next() {
                let list = &matrices[v];
                let item = if list.is_empty() {
                    None
                } else {
                    let variants = plants::VARIANTS as usize;
                    // Far tufts: seen against the lawn's texture at a distance they read as pale
                    // straw specks in the near tufts' colour; deeper and greener, they melt in.
                    let c = job.color;
                    let color = if v >= variants {
                        [c[0] * 0.72, c[1] * 0.85, c[2] * 0.78]
                    } else {
                        c
                    };
                    self.flora
                        .tinted(r, LAWN_SPECIES, job.class, (v % variants) as u32, color)
                        .map(|parts| {
                            // Levels switch by 8 m cluster: the coarsest level (a few cards)
                            // read as paler blocks across a large lawn, so lawns stop at the
                            // middle one; the last level drawn ends at `end`, then nothing.
                            let end = if job.follow {
                                LAWN_FADE_M + 2.0 * LAWN_EYE_CELL_M
                            } else {
                                LAWN_FADE_M
                            };
                            let mut ds: Vec<f32> = parts.iter().map(|p| p.2).collect();
                            ds.sort_by(f32::total_cmp);
                            ds.dedup();
                            if ds.len() > 2 {
                                ds.pop();
                            }
                            let last = ds.last().copied().unwrap_or(f32::INFINITY);
                            let parts: Vec<_> = parts
                                .iter()
                                .filter(|p| p.2 <= last)
                                .map(|&(m, mat, d)| {
                                    (m, mat, if d >= last { end } else { d.min(end) })
                                })
                                .collect();
                            let item = r.add_lod_item(&parts, list);
                            r.set_item_cull_fade(item, LAWN_FADE_BAND_M);
                            item
                        })
                };
                job.built.push(item);
                if job.built.len() < matrices.len() {
                    continue;
                }
            }
            let Some(job) = self.lawn_jobs.remove(&id) else {
                continue;
            };
            if let Some(old) = self.lawns.remove(&id) {
                old.items.into_iter().for_each(|i| r.remove_item(i));
            }
            let items = job.built.into_iter().flatten().collect();
            self.lawns.insert(
                id,
                LawnGpu {
                    key: job.key,
                    items,
                },
            );
        }
        !self.lawn_jobs.is_empty()
    }

    /// Lawns still being scattered.
    pub fn lawns_pending(&self) -> usize {
        self.lawn_jobs.len()
    }

    /// Every lawn's tufts, drawn and half drawn.
    fn drop_lawns(&mut self, r: &mut Renderer) {
        for (_, l) in self.lawns.drain() {
            l.items.into_iter().for_each(|i| r.remove_item(i));
        }
        for (_, j) in self.lawn_jobs.drain() {
            j.drop_items(r);
        }
    }

    /// Denser, finer lawns (HIGH, ULTRA) or the lighter default. True when it changed: the
    /// lawns are then dropped, for the next sync to make again.
    pub fn set_dense_lawns(&mut self, r: &mut Renderer, dense: bool) -> bool {
        if self.dense_lawns == dense {
            return false;
        }
        self.dense_lawns = dense;
        self.drop_lawns(r);
        true
    }

    /// The lawn tuft budget as a multiple of the quality's own (ULTRA and fast GPUs draw more).
    /// True when it changed: the lawns are then dropped, for the next sync to make again.
    pub fn set_lawn_budget(&mut self, r: &mut Renderer, multiple: f32) -> bool {
        if self.lawn_budget == multiple {
            return false;
        }
        self.lawn_budget = multiple;
        self.drop_lawns(r);
        true
    }

    /// The colour of a lawn's blades: the ground they stand in, in the season.
    fn blade_color(&self, m: &Material, month: u32) -> [f32; 3] {
        let ground = if material_library::overlay(&self.library, m).is_some() {
            LAWN_PICTURE_MEAN
        } else {
            [m.base_color[0], m.base_color[1], m.base_color[2]]
        };
        let t = lawn_season(month);
        [0, 1, 2].map(|k| ground[k] * t[k] * LAWN_BLADE_LIFT)
    }

    /// 3D grass on every face whose material is a lawn: tufts of `flora:lawn-tuft` scattered with
    /// the core's `lawn_scatter`, an LOD item that stops drawing at `LAWN_FADE_M`.
    fn sync_lawns(&mut self, r: &mut Renderer, project: &Project) {
        let month = project.plant_month();
        let mut live: Vec<String> = Vec::new();
        let Some(res) = plants::resolve(project, LAWN_SPECIES) else {
            return;
        };
        let class = plants::class(&res.params, month);
        // Grass thins under the crowns of trees (as grown for the years shown): their circles,
        // a little inside the drip line.
        let crowns: Vec<[f32; 3]> = tree_crowns(project)
            .into_iter()
            .map(|(_, c)| [c[0], c[1], c[2] * 0.4])
            .collect();
        let mut owners: Vec<&String> = self.owners.keys().collect();
        owners.sort();
        type Faces = Vec<(usize, visioncraft_core::material::Lawn3d, [f32; 3])>;
        let mut todo: Vec<(String, Faces)> = Vec::new();
        // Deep snow buries the lawn's blades: only the snow over the lawn shows.
        let buried = project.snow() > LAWN_SNOW_BURIES;
        let (max_density, max_tufts, narrow) = if self.dense_lawns {
            (LAWN_MAX_DENSITY * 2.0, LAWN_MAX_TUFTS * 4, 0.7)
        } else {
            (LAWN_MAX_DENSITY, LAWN_MAX_TUFTS, 1.0)
        };
        let max_tufts = (max_tufts as f32 * self.lawn_budget.max(1.0)) as usize;
        self.lawns_follow = false;
        for id in owners {
            let o = &self.owners[id];
            if !o.key.visible || !o.is_shape || buried {
                continue;
            }
            let faces: Faces = resolve_materials(project, id, &o.geom)
                .into_iter()
                .enumerate()
                .filter_map(|(f, m)| {
                    let m = project.materials.get(m?)?;
                    match &m.kind {
                        SurfaceKind::Lawn3d(l) => {
                            Some((f, *l, self.blade_color(m, u32::from(project.plant_month()))))
                        }
                        _ => None,
                    }
                })
                .collect();
            if !faces.is_empty() {
                todo.push((id.clone(), faces));
            }
        }
        let on_ground = self.ground_faces.clone();
        for (id, faces) in todo {
            live.push(id.clone());
            let o = &self.owners[&id];
            // A lawn whose tufts are over budget gets them around the eye (`lawn_scatter_within`).
            let areas: Vec<f32> = faces
                .iter()
                .map(|(f, _, _)| lawn_area(&o.geom, *f))
                .collect();
            let need: f32 = faces
                .iter()
                .zip(&areas)
                .map(|((_, l, _), a)| l.density_per_m2.min(max_density) * a)
                .sum();
            let follow = need > max_tufts as f32;
            self.lawns_follow |= follow;
            // No grass under a terrace, path or pond drawn on top of this lawn.
            // Nor under a model's faces lying on the ground (`ground_faces`).
            let mut over = areas_over(&project.shapes, &id);
            over.extend(on_ground.iter().map(|f| f.as_slice()));
            let over_key: Vec<u32> = over
                .iter()
                .flat_map(|o| o.iter().flat_map(|p| [p[0].to_bits(), p[1].to_bits()]))
                .chain(crowns.iter().flat_map(|c| c.map(f32::to_bits)))
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
                // Only the tuft species' own readiness: any other species finishing (the
                // flora's revision) made every lawn again, 50 ms each, through a load.
                u64::from(self.flora.ready(LAWN_SPECIES, class)),
                month,
                over_key,
                follow.then_some(self.lawn_eye),
            );
            if self.lawns.get(&id).is_some_and(|g| g.key == key)
                || self.lawn_jobs.get(&id).is_some_and(|j| j.key == key)
            {
                continue;
            }
            if self.flora.parts(LAWN_SPECIES, class, 0).is_none() {
                self.flora
                    .want(LAWN_SPECIES, None, class, month, &res.params);
                continue;
            }
            // The tufts are scattered off the frame (a thread; on the web at once) and drawn
            // when ready (`finish_lawns`): a dense lawn is a quarter of a million of them.
            let window = follow.then(|| {
                let c = self.lawn_eye.map(|k| (k as f32 + 0.5) * LAWN_EYE_CELL_M);
                let local = o.xf.inverse().transform_point3(Vec3::new(c[0], 0.0, c[1]));
                LawnWindow {
                    center: [local.x, local.z],
                    radius: LAWN_FADE_M + LAWN_EYE_CELL_M,
                }
            });
            let total: f32 = areas.iter().sum::<f32>().max(1e-6);
            // shortcut: one colour per lawn (its first lawn face's), fine until lawns mix colours
            let color = faces[0].2;
            let (geom, xf) = (o.geom.clone(), o.xf);
            let over: Vec<Vec<[f32; 2]>> = over.iter().map(|a| a.to_vec()).collect();
            let crowns = crowns.clone();
            let slot = plants::spawn(move || {
                // One matrix list per variant, so the lawn does not repeat one tuft; a lawn
                // scattered round the eye has a second set for its thinned, distant tufts
                // (`LAWN_FAR_M`), drawn in a deeper green.
                let mut by_variant: Vec<Vec<Mat4>> =
                    vec![Vec::new(); 2 * plants::VARIANTS as usize];
                let mut n = 0usize;
                for ((face, lawn, _), area) in faces.iter().zip(&areas) {
                    let lawn = visioncraft_core::material::Lawn3d {
                        density_per_m2: lawn.density_per_m2.min(max_density),
                        ..*lawn
                    };
                    // The budget shared by the lawn's faces by area.
                    let max = (max_tufts as f32 * area / total) as usize;
                    // An area's matrix only lifts it: the outlines hold in its own x, z.
                    let holes: Vec<&[[f32; 2]]> = over.iter().map(Vec::as_slice).collect();
                    for t in lawn_scatter_within(&geom, *face, &lawn, window, max, &holes) {
                        let p = xf.transform_point3(Vec3::from(t.position));
                        // Under a crown, three tufts in five go (shade): deterministic by place.
                        let shaded = crowns
                            .iter()
                            .any(|c| (p.x - c[0]).powi(2) + (p.z - c[1]).powi(2) < c[2] * c[2]);
                        let hash = ((p.x * 7919.0) as i32 ^ (p.z * 104729.0) as i32) as u32;
                        if shaded && hash.wrapping_mul(2654435761) % 5 < 3 {
                            continue;
                        }
                        let s = (t.height_m / 0.08).clamp(0.3, 2.5);
                        // Mowing stripes: each band's blades lean the way the mower went.
                        let lean = if lawn.stripes_m > 0.0 {
                            let a = lawn.stripes_deg.to_radians();
                            let along = Vec3::new(a.sin(), 0.0, -a.cos());
                            let across = Vec3::new(-along.z, 0.0, along.x);
                            let band = (p.dot(across) / lawn.stripes_m).floor() as i64;
                            let sign = if band.rem_euclid(2) == 0 { 1.0 } else { -1.0 };
                            Mat4::from_axis_angle(Vec3::Y.cross(along).normalize(), 0.3 * sign)
                        } else {
                            Mat4::IDENTITY
                        };
                        let far = window.is_some_and(|w| {
                            let local = xf.inverse().transform_point3(p);
                            (local.x - w.center[0]).hypot(local.z - w.center[1]) > LAWN_FAR_M
                        });
                        let k = n % plants::VARIANTS as usize
                            + if far { plants::VARIANTS as usize } else { 0 };
                        by_variant[k].push(
                            Mat4::from_translation(p)
                                * lean
                                * Mat4::from_rotation_y(t.rotation_y_deg.to_radians())
                                * Mat4::from_scale(Vec3::new(s * narrow, s, s * narrow)),
                        );
                        n += 1;
                    }
                }
                by_variant
            });
            let old = self.lawn_jobs.insert(
                id,
                LawnJob {
                    key,
                    class,
                    color,
                    slot,
                    matrices: None,
                    built: Vec::new(),
                    follow,
                },
            );
            // A job replaced half drawn: its items go too (they stayed, tufts of a lawn that
            // was no longer there, over a terrace or a pond).
            old.into_iter().for_each(|j| j.drop_items(r));
        }
        let stale: Vec<String> = self
            .lawn_jobs
            .keys()
            .filter(|k| !live.contains(k))
            .cloned()
            .collect();
        for id in stale {
            if let Some(j) = self.lawn_jobs.remove(&id) {
                j.drop_items(r);
            }
        }
        self.finish_lawns(r);
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
        let month = project.plant_month();
        for g in &project.generators {
            let sig = (
                g.clone(),
                month,
                project.layer_visible(g.layer()),
                project.view.years.map(f32::to_bits),
            );
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
                && !self.growth_stale(project, month, &o.groups)
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

    /// While the project shows years after planting: the growth stage a planting's plants of
    /// `asset` are generated at (tenths of maturity; none: full grown) and the scale that brings
    /// them from it to their exact size. Catalogue models only scale.
    fn growth_stage(
        &self,
        project: &Project,
        assets: &[Asset],
        g: &Generator,
        asset: &str,
    ) -> (Option<u8>, f32, bool) {
        let Some(years) = project.view.years else {
            return (None, 1.0, false);
        };
        let size = self.custom_sizes.get(asset).copied().or_else(|| {
            assets
                .iter()
                .find(|a| a.id == asset)
                .map(|a| (a.size_m[1], a.size_m[0].max(a.size_m[2])))
        });
        let growth = visioncraft_core::growth::growth_of(asset, size);
        let planted = g.planted_age_years().unwrap_or(growth.planted_age_years);
        let s = growth.size(planted + years).height;
        let flora = !self.library.knows(asset) && plants::resolve(project, asset).is_some();
        if !flora {
            return (None, s, false);
        }
        let tenth = ((s * 10.0).round() as u8).clamp(1, 10);
        ((tenth < 10).then_some(tenth), s, true)
    }

    /// The stage to draw a group at: the wanted one when generated, else the nearest one that
    /// is (scaled to size meanwhile, so dragging the years is smooth), and the scale to apply.
    fn stage_to_draw(
        &mut self,
        project: &Project,
        asset: &str,
        variant: u32,
        month: u8,
        (want, s, flora): (Option<u8>, f32, bool),
    ) -> (Option<u8>, f32) {
        if !flora {
            return (None, s);
        }
        let ready = |me: &Self, a: Option<u8>| {
            me.source(project, asset, variant, When { month, age: a })
                .is_some()
        };
        let tenths = |a: Option<u8>| f32::from(a.unwrap_or(10));
        let chosen = if ready(self, want) {
            want
        } else {
            // Ask for the wanted stage, draw the nearest ready one meanwhile.
            if let Some(res) = plants::resolve(project, asset) {
                let (class, params) = When { month, age: want }.class(&res.params);
                self.flora
                    .want(asset, res.scan_id.as_deref(), class, month, &params);
            }
            let mut stages: Vec<Option<u8>> = (1..10).map(Some).chain([None]).collect();
            stages.sort_by(|a, b| {
                (tenths(*a) - tenths(want))
                    .abs()
                    .total_cmp(&(tenths(*b) - tenths(want)).abs())
            });
            stages.into_iter().find(|a| ready(self, *a)).unwrap_or(want)
        };
        (chosen, s / (tenths(chosen) / 10.0))
    }

    /// Whether a group draws a stand-in stage while its wanted one has since been generated.
    fn growth_stale(
        &self,
        project: &Project,
        month: u8,
        groups: &HashMap<(String, u32), Group>,
    ) -> bool {
        groups.iter().any(|((asset, variant), grp)| {
            grp.age != grp.want
                && self
                    .source(
                        project,
                        asset,
                        *variant,
                        When {
                            month,
                            age: grp.want,
                        },
                    )
                    .is_some()
        })
    }

    /// What a group draws as its real self: a catalogue asset's resident LOD parts (the
    /// catalogue wins over a plant of the same id), else the generated plant. `None` while only a
    /// stand-in can show.
    fn source(&self, project: &Project, asset: &str, variant: u32, when: When) -> Option<LodParts> {
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
        let (class, _) = when.class(&res.params);
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
        when: When,
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
            let impostor = self.impostor_for(project, asset, variant, when);
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
                let (class, params) = when.class(&res.params);
                self.flora
                    .want(asset, res.scan_id.as_deref(), class, when.month, &params);
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
        when: When,
    ) -> Option<(ImpostorId, f32, bool)> {
        if self.library.knows(asset) {
            return None;
        }
        let res = plants::resolve(project, asset)?;
        let (class, _) = when.class(&res.params);
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
            let when = When {
                month,
                age: grp.age,
            };
            let src = self.source(project, asset, *variant, when);
            let imp = self
                .impostor_for(project, asset, *variant, when)
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
                when,
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
        sig: (Generator, u8, bool, Option<u32>),
        bounds: Option<Rect>,
        old: Option<GenGpu>,
    ) -> GenGpu {
        let t0 = plant_scene::now_ms();
        // A planting on a hidden layer is evaluated all the same (the panel counts its plants)
        // and draws nothing.
        let e = self.evaluate_generator(g);
        let (count, truncated) = (e.placements.len(), e.truncated);
        let placements = if sig.2 { e.placements } else { Vec::new() };
        let t1 = plant_scene::now_ms();
        let month = sig.1;
        let stamp = self.dirty.stamp();
        let mut old_groups = old.map(|o| o.groups).unwrap_or_default();
        let mut groups: HashMap<(String, u32), Group> = HashMap::new();
        self.library.forget_generator(g.id());
        for (asset, variant, matrices) in
            plant_scene::group_placements(&placements, plants::VARIANTS)
        {
            // Grown for the years shown: a growth stage to generate at, and the scale to it.
            let stage = self.growth_stage(project, assets, g, &asset);
            let want = stage.0;
            let (age, k) = self.stage_to_draw(project, &asset, variant, month, stage);
            let matrices: Vec<Mat4> = if k == 1.0 {
                matrices
            } else {
                matrices
                    .into_iter()
                    .map(|m| m * Mat4::from_scale(Vec3::splat(k)))
                    .collect()
            };
            let when = When { month, age };
            if self.library.knows(&asset) {
                // the library streams the level the nearest instance needs
                self.library.note(g.id(), &asset, &matrices);
            }
            let src = self.source(project, &asset, variant, when);
            let imp = self
                .impostor_for(project, &asset, variant, when)
                .map(|i| i.0);
            let items = match old_groups.remove(&(asset.clone(), variant)) {
                Some(prev) if prev.parts == src && prev.impostor == imp && prev.age == age => {
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
                        when,
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
                    age,
                    want,
                },
            );
        }
        Self::drop_groups(r, old_groups);
        let insts = plant_scene::instances(project, assets, &placements);
        let empty = (count == 0).then(|| self.why_empty(g)).flatten();
        let hidden = !sig.2;
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
            empty,
            count,
            hidden,
        }
    }

    /// Why generator `g` places no plant: evaluated again without the building rule and the
    /// area it keeps inside, the first that brings plants back is the reason.
    pub fn why_empty(&self, g: &Generator) -> Option<&'static str> {
        let without = |keys: &[&str]| {
            let mut v = serde_json::to_value(g).ok()?;
            let o = v.as_object_mut()?;
            for k in keys {
                match o.get(*k) {
                    Some(serde_json::Value::Bool(_)) => o.insert((*k).into(), false.into()),
                    Some(serde_json::Value::Array(_)) => {
                        o.insert((*k).into(), serde_json::json!([]))
                    }
                    _ => None,
                };
            }
            let g: Generator = serde_json::from_value(v).ok()?;
            Some(!self.evaluate_generator(&g).placements.is_empty())
        };
        if without(&["avoid_buildings", "clip_outline", "clip_holes"]) != Some(true) {
            return Some(crate::strings::EMPTY_DENSITY);
        }
        if without(&["clip_outline", "clip_holes"]) == Some(true) {
            return Some(crate::strings::EMPTY_OUTSIDE);
        }
        Some(crate::strings::EMPTY_BUILDINGS)
    }

    /// The terrain's painted layers from their materials (pictures arrive over time).
    pub fn sync_terrain_layers(&mut self, r: &mut Renderer, project: &Project) {
        let month = u32::from(project.plant_month());
        self.terrain_gpu
            .sync_layers(r, project, &mut self.library, &mut |m, lib| {
                // Mowing stripes belong to the drawn lawn, not the ground round it.
                Look {
                    stripes: 0,
                    ..look_of(m, lib, month)
                }
            });
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
            let look = Look {
                stripes: 0,
                ..look_of(m, &mut self.library, month)
            };
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
        self.flora.scans.pump(&mut self.library.store);
        self.finish_lawns(r);
        let cell = [eye.x, eye.z].map(|v| (v / LAWN_EYE_CELL_M).floor() as i32);
        let moved = std::mem::replace(&mut self.lawn_eye, cell) != cell && self.lawns_follow;
        self.flora.pump(r) | library | moved
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
            count: g.count,
            truncated: g.truncated,
            empty: g.empty,
            hidden: g.hidden,
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

    /// The colour (linear albedo) of what lies under `at`: the material of the first surface
    /// below, else the terrain's base layer; `None` over nothing. For the light the ground
    /// bounces up.
    pub fn albedo_below(&self, project: &Project, at: Vec3) -> Option<[f32; 3]> {
        let colour = |id: &str| {
            project
                .materials
                .iter()
                .find(|m| m.id == id)
                .map(|m| [m.base_color[0], m.base_color[1], m.base_color[2]])
        };
        if let Some(p) = self.raycast(at, Vec3::NEG_Y, false)
            && let Some(o) = self.owners.get(&p.owner)
        {
            let index = resolve_materials(project, &p.owner, &o.geom)
                .get(p.face)
                .copied()
                .flatten();
            if let Some(m) = index.and_then(|i| project.materials.get(i)) {
                return Some([m.base_color[0], m.base_color[1], m.base_color[2]]);
            }
        }
        project
            .terrain
            .as_ref()
            .and_then(|t| t.layers.first())
            .and_then(|l| colour(&l.material))
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
        let a = key(1, vec![[0; 14]], true);
        let (i, t) = (Mat4::IDENTITY, Mat4::from_translation(Vec3::X));
        assert_eq!(classify(&a, &a.clone(), &i, &i), Change::Keep);
        assert_eq!(classify(&a, &a.clone(), &i, &t), Change::Transform);
        assert_eq!(
            classify(&a, &key(2, vec![[0; 14]], true), &i, &i),
            Change::Rebuild
        );
        assert_eq!(
            classify(&a, &key(1, vec![[1; 14]], true), &i, &i),
            Change::Rebuild
        );
        assert_eq!(
            classify(&a, &key(1, vec![[0; 14]], false), &i, &t),
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
