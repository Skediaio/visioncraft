//! Turns the core `Project` plus loaded geometry into renderer meshes, materials and items, and
//! updates only what changed. The only place that builds `visioncraft_render::Mesh`/`Material`.

use glam::{Mat4, Vec3};
use std::collections::HashMap;
use std::sync::Arc;
use visioncraft_core::grass;
use visioncraft_core::material::Lawn3d;
use visioncraft_core::wall_photo::{Corner, face_frame, rectified_size, split_by_region};
use visioncraft_core::{
    Asset, Evaluation, Generator, ImportedModel, LawnWindow, Material, ModelRef, Project, Shape,
    SurfaceKind, Terrain, area_lifts, areas_over, blends_over, blocked_by_buildings,
    evaluate_in_scene, evaluate_surface_fill, ground_height, inside_outline, lawn_area,
    lawn_scatter_blended, placed_shape, raycast, resolve_materials, shape_mesh,
    shape_mesh_on_terrain_with, water_areas,
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
mod natural;

/// A loaded or generated design, with a counter that changes when its triangles do.
#[derive(Clone)]
pub struct Loaded {
    pub model: Arc<ImportedModel>,
    pub version: u64,
}

pub type LookKey = [u32; 23];

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
    /// How strongly dirt, moss, wear and streaks age the surface (`Weathering::strengths`: the
    /// material's own and the view's) and true north on the ground (x, z) for the moss.
    pub aging: [f32; 4],
    pub north: [f32; 2],
    /// 0 standard, 1 glass, 2 water.
    pub surface: u32,
    pub variation: f32,
    /// Metres per texture repeat.
    pub scale_m: f32,
    /// Index of the material's maps in the library (0: none).
    pub tex: u32,
    /// Mowing stripes, packed as the renderer takes them (0: none).
    pub stripes: u32,
    /// Depth of the height map's relief in metres (0: off).
    pub parallax_m: f32,
    /// A straightened wall photo, drawn by its own UVs: 1 + its index in `Scene::photo_faces`
    /// (0: none). Only the pieces of a photo's region carry it; a face's own look never does.
    pub photo: u32,
    /// Puddles may form on the surface when it is wet: false for a lawn (it stays damp, but
    /// holds no standing water).
    pub puddles: bool,
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
            aging: [0.0; 4],
            north: [0.0, -1.0],
            surface: 0,
            variation: 0.0,
            scale_m: 1.0,
            tex: 0,
            stripes: 0,
            parallax_m: 0.0,
            photo: 0,
            puddles: true,
        }
    }

    /// The look of the region of wall photo number `photo` (1 + its index): the picture decides.
    pub const fn of_photo(photo: u32) -> Self {
        Self {
            photo,
            ..Self::plain(1.0, 1.0, 1.0, 0.85)
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
            self.parallax_m,
            f32::from_bits(self.photo),
            self.aging[0],
            self.aging[1],
            self.aging[2],
            self.aging[3],
            self.north[0],
            self.north[1],
            self.puddles as u32 as f32,
        ]
        .map(f32::to_bits)
    }
}

const CLAY: Look = Look::plain(0.85, 0.85, 0.85, 0.9);
/// The one look of surroundings (owners on a context layer): a light, slightly warm grey, like
/// clay but per owner. Their material assignments are ignored.
const MASSING: Look = Look::plain(0.84, 0.82, 0.78, 0.9);
const UNASSIGNED: Look = Look::plain(0.6, 0.6, 0.6, 0.8);

/// The look that replaces an owner's materials altogether: the massing look on a context layer
/// (surroundings are not part of the design), else none. The one place this rule lives.
fn forced_look(project: &Project, owner: &str) -> Option<Look> {
    project.owner_is_context(owner).then_some(MASSING)
}

/// The variation a material shows: its own, else (an untouched palette material) its library
/// look's.
pub fn shown_variation(lib: &Library, m: &Material) -> f32 {
    if m.variation > 0.0 || !library::spec_of(&m.maps).is_empty() {
        return m.variation;
    }
    material_library::overlay(lib, m).map_or(0.0, |o| o.variation)
}

/// True north on the ground as a drawing direction (x, z), turned to the south in the southern
/// hemisphere: the side of a wall the sun does not reach, where moss grows.
fn shade_direction(site: &visioncraft_core::Site) -> [f32; 2] {
    let b = site.bearing_to_drawing(if site.latitude < 0.0 { 180.0 } else { 0.0 });
    [b[0], b[2]]
}

/// `view_weathering`: the view's, added to the material's own (`Weathering::strengths`).
fn look_of(
    m: &Material,
    lib: &mut Library,
    month: u32,
    site: &visioncraft_core::Site,
    view_weathering: f32,
) -> Look {
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
            parallax_depth_m,
        } => Look {
            relief: ov.as_ref().map_or(*relief, |o| o.relief),
            // An overlaid palette material takes its library kind's depth unless it sets one.
            parallax_m: match &ov {
                Some(o) if *parallax_depth_m == 0.0 => o.parallax_m,
                _ => *parallax_depth_m,
            },
            gloss: ov.as_ref().map_or(*gloss, |o| o.gloss),
            weathering: *weathering,
            aging: visioncraft_core::material::Weathering::strengths(m.weathering, view_weathering),
            north: shade_direction(site),
            scale_m: ov.as_ref().map_or(*map_scale_m, |o| o.scale_m),
            ..plain
        },
        // The tufts or the GPU grass are made separately (`Scene::sync_lawns`); the ground under
        // them is this.
        SurfaceKind::Lawn3d(l) => {
            let t = lawn_season(month);
            // Under long grass the ground is the sward's shaded floor, darker than a mown lawn's.
            let floor = if l.blade_height_m > MEADOW_FROM_M {
                0.6
            } else {
                1.0
            };
            let c = plain.color.map(|k| k * floor);
            Look {
                color: [c[0] * t[0], c[1] * t[1], c[2] * t[2], plain.color[3]],
                variation: plain.variation.max(0.6),
                scale_m: ov.as_ref().map_or(1.0, |o| o.scale_m),
                relief: ov.as_ref().map_or(0.0, |o| o.relief),
                gloss: ov.as_ref().map_or(0.0, |o| o.gloss),
                stripes: stripes_packed(&turned_lawn(l, site)),
                puddles: false,
                ..plain
            }
        }
    }
}

/// The lawn with its stripe bearing (true, clockwise from north) turned into the drawing.
fn turned_lawn(
    l: &visioncraft_core::material::Lawn3d,
    site: &visioncraft_core::Site,
) -> visioncraft_core::material::Lawn3d {
    visioncraft_core::material::Lawn3d {
        stripes_deg: site.bearing_to_drawing_deg(f64::from(l.stripes_deg)) as f32,
        ..*l
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
        aging: look.aging,
        aging_north: look.north,
        variation: look.variation,
        stripes: look.stripes,
        puddles: look.puddles,
        parallax_depth_m: look.parallax_m,
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

/// A plant mesh part (positions, normals, UVs, wind) into the renderer; inside a crown (the
/// bounds of a plant's foliage) with the sky occlusion of its place there (`crown_occlusion`):
/// the leaves, and the branches among them (`wood`), which read bright salmon in the crown's
/// shade without it. Outside a crown, branches and a clipped hedge's leaves (`hedge`) see the
/// whole sky, tagged as a plant's all the same: snow settles on them as on a crown's top, and no
/// snow band is drawn under a bend of a trunk as under a sill.
pub fn upload_plant_mesh(
    r: &mut Renderer,
    p: &flora::MeshPart,
    crown: Option<(Vec3, Vec3)>,
    wood: bool,
    hedge: bool,
) -> Option<MeshId> {
    let occlusion = match crown {
        Some(b) => Some(crown_occlusion(&p.positions, b, wood)),
        None if wood || hedge => Some(vec![CROWN_TAG | 65535; p.positions.len()]),
        None => None,
    };
    upload_plant_mesh_with(r, p, occlusion)
}

/// A lawn tuft (`tuft_occlusion`).
pub fn upload_tuft_mesh(r: &mut Renderer, p: &flora::MeshPart) -> Option<MeshId> {
    upload_plant_mesh_with(r, p, Some(tuft_occlusion(&p.positions)))
}

fn upload_plant_mesh_with(
    r: &mut Renderer,
    p: &flora::MeshPart,
    occlusion: Option<Vec<u32>>,
) -> Option<MeshId> {
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

/// The bounds of a mesh part's positions.
pub fn part_bounds(positions: &[[f32; 3]]) -> (Vec3, Vec3) {
    positions.iter().fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(lo, hi), p| (lo.min(Vec3::from(*p)), hi.max(Vec3::from(*p))),
    )
}

/// Tag of a face id that carries a leaf's sky occlusion in its low 16 bits (scene.wgsl's
/// `CROWN_TAG`); plain face ids are small numbers.
const CROWN_TAG: u32 = 0xC000_0000;

/// Tag of a lawn tuft's baked occlusion (scene.wgsl's `TUFT_TAG`): the same 16 bits as a crown's.
const TUFT_TAG: u32 = 0x8000_0000;

/// Per vertex of a grass tuft, the sky its place in the sward sees: the base is shaded by the
/// blades round it (near two fifths of the sky), the tips see all of it. Baked ambient occlusion
/// in the grass, as Horizon Zero Dawn's artists did (Guerrilla Games, "The Real-Time Volumetric
/// Cloudscapes / Nubis / ... Horizon Zero Dawn" GDC talks on the ground cover: AO painted into
/// the blades' vertices); the 0.4 floor and the rise over the lower 70 % are guesses.
pub fn tuft_occlusion(positions: &[[f32; 3]]) -> Vec<u32> {
    let (lo, hi) = positions
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p[1]), hi.max(p[1]))
        });
    let span = (hi - lo).max(1e-4);
    positions
        .iter()
        .map(|p| {
            let t = ((p[1] - lo) / span / 0.7).clamp(0.0, 1.0);
            let sky = 0.4 + 0.6 * t * t * (3.0 - 2.0 * t);
            TUFT_TAG | (sky * 65535.0) as u32
        })
        .collect()
}

/// Per leaf vertex, how much sky it sees through the crown, as a tagged face id: leaves deep
/// under the crown's top and inside it see little (an ellipsoid round the leaves' bounds; light
/// falls through a crown in a few layers of leaves). The tops keep most of it, the bottom inside down
/// to about two fifths, so a crown seen from above is not one pale, evenly lit surface; the sun
/// on a leaf is dimmed by it too (scene.wgsl `light`).
/// Branches (`wood`) outside the crown's ellipsoid see the whole sky.
pub fn crown_occlusion(positions: &[[f32; 3]], (lo, hi): (Vec3, Vec3), wood: bool) -> Vec<u32> {
    let (c, rad) = ((lo + hi) * 0.5, ((hi - lo) * 0.5).max(Vec3::splat(1e-3)));
    positions
        .iter()
        .map(|p| {
            let q = (Vec3::from(*p) - c) / rad;
            let top = c.y + rad.y * (1.0 - q.x * q.x - q.z * q.z).max(0.0).sqrt();
            let below = ((top - p[1]) / (2.0 * rad.y)).clamp(0.0, 1.0);
            let inside = (1.0 - q.length()).clamp(0.0, 1.0);
            if wood && inside <= 0.0 {
                return CROWN_TAG | 65535;
            }
            let s = below.min(0.75) / 0.75;
            // Even the top leaves have neighbours that hide part of the sky.
            // Light also reaches the lower crown through the thin leaves above it: a floor of
            // about two fifths, so a crown's underside is shaded green, not black.
            let sky = (0.88 - 0.36 * s * s * (3.0 - 2.0 * s) - 0.15 * inside).max(0.42);
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
    upload_uvs(r, d, face_ids, None)
}

/// Triangles grouped by look: the look, whether it is a soft edge band, the mesh data, the UVs.
type Groups = Vec<(Look, bool, MeshData, Vec<[f32; 2]>)>;

/// Adds a triangle of `face` to the group of `look` (made when new), with its UVs when it has any.
#[allow(clippy::too_many_arguments)]
fn push_triangle(
    groups: &mut Groups,
    by_key: &mut HashMap<(LookKey, bool), usize>,
    face_group: &mut [Option<usize>],
    face: usize,
    look: Look,
    soft: bool,
    corners: [Corner; 3],
    uvs: Option<[[f32; 2]; 3]>,
) {
    let g = *by_key.entry((look.key(), soft)).or_insert_with(|| {
        groups.push((look, soft, MeshData::default(), Vec::new()));
        groups.len() - 1
    });
    face_group[face] = Some(g);
    let (_, _, d, group_uvs) = &mut groups[g];
    for (i, c) in corners.iter().enumerate() {
        d.indices.push(d.positions.len() as u32);
        d.positions.push(c.position);
        d.normals.push(c.normal);
        d.face_ids.push(face as u32);
        if let Some(uvs) = uvs {
            group_uvs.push(uvs[i]);
        }
    }
}

fn upload_uvs(
    r: &mut Renderer,
    d: &MeshData,
    face_ids: bool,
    uvs: Option<&[[f32; 2]]>,
) -> Option<MeshId> {
    let mesh = visioncraft_render::Mesh {
        positions: &d.positions,
        normals: &d.normals,
        indices: &d.indices,
        face_ids: face_ids.then_some(d.face_ids.as_slice()),
        uvs,
        wind: None,
    };
    r.add_mesh(&mesh).ok()
}

#[derive(Clone, Debug, PartialEq)]
pub enum Geom {
    /// The loaded model's version and the bodies hidden in it.
    Model(u64, Vec<String>),
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
    /// Per face: its wall photo (1 + its index in `Scene::photo_faces`), 0 for none; empty when
    /// no face has one.
    pub photos: Vec<u32>,
}

/// A wall photo ready to draw on a face: the straightened picture and where it lies on the face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhotoFace {
    pub texture: visioncraft_render::TextureId,
    /// The face's frame in the owner's own space.
    pub frame: visioncraft_core::wall_photo::FaceFrame,
    /// The rectangle of the frame the picture covers.
    pub region: visioncraft_core::wall_photo::Region,
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
    Vec<(usize, [u32; 6], u64)>,
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

/// A lawn's tufts to scatter on its shape (`scatter_lawn`): plain data, so that on the web a
/// texture worker scatters them (a lawn scattered round the eye again each time the eye moved
/// a cell held the page for 50 ms a frame while walking).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct LawnScatter {
    /// The shape's matrix (columns).
    pub xf: [f32; 16],
    /// (face, its lawn) and each face's area.
    pub faces: Vec<(usize, visioncraft_core::material::Lawn3d)>,
    pub areas: Vec<f32>,
    pub max_density: f32,
    pub max_tufts: usize,
    /// Blade width factor (dense lawns' blades are narrower).
    pub narrow: f32,
    /// Round the eye (`LawnWindow`: centre, radius), for a lawn over its budget.
    pub window: Option<([f32; 2], f32)>,
    /// Outlines without grass (terraces, paths, ponds over it), in the shape's x, z.
    pub over: Vec<Vec<[f32; 2]>>,
    /// The soft edge width of each of `over` (`Area::edge_blend_m`), 0 where missing: tufts
    /// carry on into the band (`lawn_scatter_blended`).
    #[serde(default)]
    pub over_blend: Vec<f32>,
    /// Tree crowns' circles (x, z, radius), where grass thins.
    pub crowns: Vec<[f32; 3]>,
}

/// The tufts' matrices, one list per variant (so the lawn does not repeat one tuft); a lawn
/// scattered round the eye has a second set for its thinned, distant tufts (`LAWN_FAR_M`), drawn
/// in a deeper green.
pub fn scatter_lawn(geom: &ImportedModel, job: &LawnScatter) -> Vec<Vec<Mat4>> {
    let xf = Mat4::from_cols_array(&job.xf);
    let window = job
        .window
        .map(|(center, radius)| LawnWindow { center, radius });
    let total: f32 = job.areas.iter().sum::<f32>().max(1e-6);
    let mut by_variant: Vec<Vec<Mat4>> = vec![Vec::new(); 2 * plants::VARIANTS as usize];
    let mut n = 0usize;
    for ((face, lawn), area) in job.faces.iter().zip(&job.areas) {
        let lawn = visioncraft_core::material::Lawn3d {
            density_per_m2: lawn.density_per_m2.min(job.max_density),
            ..*lawn
        };
        // The budget shared by the lawn's faces by area.
        let max = (job.max_tufts as f32 * area / total) as usize;
        // An area's matrix only lifts it: the outlines hold in its own x, z.
        let holes: Vec<&[[f32; 2]]> = job.over.iter().map(Vec::as_slice).collect();
        for t in lawn_scatter_blended(geom, *face, &lawn, window, max, &holes, &job.over_blend) {
            let p = xf.transform_point3(Vec3::from(t.position));
            // Under a crown, three tufts in five go (shade): deterministic by place.
            let shaded = job
                .crowns
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
            let k = n % plants::VARIANTS as usize + if far { plants::VARIANTS as usize } else { 0 };
            let narrow = job.narrow;
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
}

/// Scatters a lawn off the frame: natively on the work pool, on the web in a texture worker
/// (on the page where none runs).
fn start_lawn(geom: Arc<ImportedModel>, job: LawnScatter) -> plants::Slot<Vec<Vec<Mat4>>> {
    #[cfg(not(target_arch = "wasm32"))]
    return plants::spawn(move || scatter_lawn(&geom, &job));
    #[cfg(target_arch = "wasm32")]
    {
        let slot = plants::Slot::default();
        let out = slot.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let m = match crate::texture_worker::lawn(&geom, &job).await {
                Some(m) => m,
                None => scatter_lawn(&geom, &job),
            };
            if let Ok(mut s) = out.lock() {
                *s = Some(m);
            }
        });
        slot
    }
}

/// Grass taller than this (m) is a meadow's: a darker floor, drying from midsummer.
const MEADOW_FROM_M: f32 = 0.15;

/// A shape's lawn faces: face, its lawn (turned into the drawing), its blades' colour.
type LawnFaces = Vec<(usize, visioncraft_core::material::Lawn3d, [f32; 3])>;
/// Outlines in world x, z.
type Holes = Vec<Vec<[f32; 2]>>;
/// What a grass mask was made from (`Scene::sync_grass`).
type GrassKey = (Vec<Geom>, Vec<u32>);

/// A lawn's numbers that change its grass mask, as bits for `GrassKey`.
fn lawn_bits(l: &Lawn3d) -> [u32; 5] {
    [
        l.density_per_m2,
        l.blade_height_m,
        l.stripes_m,
        l.stripes_deg,
        l.flowers,
    ]
    .map(f32::to_bits)
}

/// The share of dead, straw-coloured blades in a month: a lawn's few all summer, more in winter;
/// a meadow's long grass (over 15 cm) dries out from midsummer, as hay.
fn grass_dry(month: u32, blade_height_m: f32) -> f32 {
    let lawn = match month {
        12 | 1 | 2 => 0.3,
        3 | 11 => 0.15,
        _ => 0.04,
    };
    let meadow = match month {
        7 | 8 => 0.18,
        9 | 10 => 0.3,
        _ => 0.0,
    };
    if blade_height_m > MEADOW_FROM_M {
        f32::max(lawn, meadow)
    } else {
        lawn
    }
}

/// How much of a meadow's flowers bloom in a month: from April, most in May to July, the last
/// in September.
fn flower_season(month: u32) -> f32 {
    match month {
        4 => 0.5,
        5..=7 => 1.0,
        8 => 0.75,
        9 => 0.35,
        _ => 0.0,
    }
}

/// The flowers of a grassland in a month (linear RGB): buttercup, daisy, cuckooflower and
/// dandelion in spring; daisy, red clover, knapweed and cornflower in summer; yarrow, knapweed,
/// hawkbit and scabious late in the summer.
fn flower_colors(month: u32) -> [[f32; 3]; 4] {
    match month {
        1..=5 => [
            [0.62, 0.42, 0.01],
            [0.72, 0.72, 0.66],
            [0.45, 0.36, 0.55],
            [0.68, 0.38, 0.0],
        ],
        6 | 7 => [
            [0.72, 0.72, 0.66],
            [0.5, 0.12, 0.25],
            [0.3, 0.09, 0.38],
            [0.08, 0.17, 0.6],
        ],
        _ => [
            [0.66, 0.63, 0.55],
            [0.3, 0.09, 0.38],
            [0.62, 0.4, 0.01],
            [0.3, 0.26, 0.55],
        ],
    }
}

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
/// LOD parts with their distances times the largest instance scale (never shortened).
fn scale_lod_distances(parts: &LodParts, matrices: &[Mat4]) -> LodParts {
    let k = matrices
        .iter()
        .map(|m| m.x_axis.length().max(m.y_axis.length()))
        .fold(1.0_f32, f32::max);
    parts
        .iter()
        .map(|&(m, mat, d)| (m, mat, (d * k).min(f32::MAX)))
        .collect()
}

/// Where a catalogue plant's coarsest model hands over to its impostor: metres (before the
/// quality's level-of-detail scale) per metre of height. At MEDIUM's half distances a plant is
/// then about 10 pixels tall: its impostor, baked from the model's twigs, fills them out to a
/// blob that showed from nearer (a 0.5 m bush at 17 m).
const PLANT_IMPOSTOR_M_PER_M: f32 = 150.0;

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
    /// The same looks as cutouts, for the soft edge bands of areas (`soft_material`).
    soft_materials: HashMap<LookKey, MaterialId>,
    /// The wall photos shown, by content (a `Look::photo` is an index + 1 into it).
    photo_faces: Vec<PhotoFace>,
    /// What the last sync asked the store for: a photo's straightened picture and the texture it
    /// had then (`photos_changed`).
    photo_wanted: Vec<(String, Option<visioncraft_render::TextureId>)>,
    /// Looks made for placeholders and the figure: never pruned.
    pinned: std::collections::HashSet<LookKey>,
    protos: HashMap<String, Vec<(MeshId, MaterialId)>>,
    /// Where the ground changed since which stamp: generators depend on it.
    dirty: DirtyLog,
    terrain: Option<Arc<Terrain>>,
    /// The terraces: inside one the ground is its floor (`visioncraft_core::terrace`).
    zones: Vec<visioncraft_core::terrace::Zone>,
    /// The project's sea level as last synced: plantings are kept off its bed.
    sea: Option<visioncraft_core::Sea>,
    terrain_gpu: crate::terrain_gpu::TerrainGpu,
    pub timings: Timings,
    pub flora: Flora,
    /// The asset library: models, textures and lamps streamed in.
    pub library: Library,
    people: Option<ItemId>,
    /// Denser, finer lawn tufts (HIGH and ULTRA; `set_dense_lawns`).
    dense_lawns: bool,
    /// The renderer draws the lawns' grass on the GPU (`Quality::grass` above 0; `set_gpu_grass`):
    /// no tufts, a grass mask instead.
    gpu_grass: bool,
    /// What the grass mask given to the renderer was made from (`sync_grass`).
    grass_key: Option<GrassKey>,
    /// The eye's cell (`LAWN_EYE_CELL_M` squares): lawns over their tuft budget are scattered
    /// around it (`lawns_follow`), and again when it moves to another cell.
    lawn_eye: [i32; 2],
    lawns_follow: bool,
    /// The eye's cell for the GPU grass of a large terrain (`grass::window_cell`): its mask is
    /// made round the eye (`grass_follow`), and again when the eye moves to another cell.
    grass_eye: [i32; 2],
    grass_follow: bool,
    /// The tuft budget's multiple of the quality's (`set_lawn_budget`): more on fast GPUs.
    lawn_budget: f32,
    /// Fallen leaves, stones, twigs and lawn edges (`clutter`).
    clutter: clutter::Clutter,
    /// LOW quality: no clutter whatever the project says (`set_clutter_low`).
    clutter_low: bool,
    /// Rocks, deadwood and the terrain's ground cover (`natural`).
    natural: natural::Natural,
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
        if look.photo != 0
            && let Some(pf) = self.photo_faces.get(look.photo as usize - 1)
        {
            // The picture decides, on the mesh's UVs; lit like any surface.
            let m = visioncraft_render::Material {
                base_color: [1.0, 1.0, 1.0, 1.0],
                metallic: 0.0,
                roughness: look.roughness,
                base_color_map: Some(pf.texture),
                relief: 0.0,
                projection: visioncraft_render::Projection::Uv,
                ..Default::default()
            };
            let id = r.add_material(&m);
            self.materials.insert(look.key(), id);
            return id;
        }
        let m = gpu_material(look);
        let id = r.add_material(&m);
        if look.tex != 0 {
            self.library.register_textured(id, m, look.tex as usize);
        }
        self.materials.insert(look.key(), id);
        id
    }

    /// The material of a look in an area's soft edge band: the same, as a cutout (the depth
    /// pre-pass dithers its coverage by the edge value in the UVs, scene.wgsl `edge_coverage`).
    /// Few, so never pruned.
    fn soft_material(&mut self, r: &mut Renderer, look: &Look) -> MaterialId {
        if let Some(id) = self.soft_materials.get(&look.key()) {
            return *id;
        }
        let mut m = gpu_material(look);
        m.alpha_mode = visioncraft_render::AlphaMode::Cutout { threshold: 0.5 };
        let id = r.add_material(&m);
        if look.tex != 0 {
            self.library.register_textured(id, m, look.tex as usize);
        }
        self.soft_materials.insert(look.key(), id);
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
            .chain(
                self.owners
                    .values()
                    .flat_map(|o| o.key.photos.iter().filter(|p| **p != 0))
                    .map(|p| Look::of_photo(*p).key()),
            )
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
        self.natural.clear(r);
        self.terrain_gpu.clear(r);
        self.terrain = None;
        self.zones.clear();
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
        // The terraces: their footprints are cut out of the terrain, and what stood on the old
        // ground there (or stands on the floor now) is placed again.
        let zones = visioncraft_core::terrace::project_zones(project);
        if self.zones != zones {
            let moved: Vec<&visioncraft_core::terrace::Zone> = self
                .zones
                .iter()
                .filter(|z| !zones.contains(z))
                .chain(zones.iter().filter(|z| !self.zones.contains(z)))
                .collect();
            for z in moved {
                let (lo, hi) = z
                    .hole
                    .iter()
                    .fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), p| {
                        (
                            [lo[0].min(p[0]), lo[1].min(p[1])],
                            [hi[0].max(p[0]), hi[1].max(p[1])],
                        )
                    });
                self.dirty.mark([lo, hi]);
            }
            self.zones = zones;
        }
        let holes = visioncraft_core::terrace::holes(&self.zones);
        self.pinned.insert(CLAY.key());
        let any = self.material(r, &CLAY); // terrain items ignore their material
        let terrain = crate::load_stats::time("terrain sync (main thread)", || {
            self.terrain_gpu.sync(
                r,
                project.terrain.as_ref(),
                project.view.ground.map(|g| g.height),
                &holes,
                any,
            )
        });
        if let Some(d) = terrain {
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
        // A new sea level re-places every planting: what it covers is kept off the bed.
        if self.sea != project.sea {
            self.sea.clone_from(&project.sea);
            self.dirty.mark([[-1e7, -1e7], [1e7, 1e7]]);
        }
        let clay = false; // the renderer's own clay mode handles it (`Renderer::set_clay`)

        // Owners: models, then shapes.
        let mut wanted: Vec<Wanted> = Vec::new();
        // shortcut: the terrain's address tells versions apart; an edit makes a new Arc
        let terrain_key = project
            .terrain
            .as_ref()
            .map_or(0, |t| Arc::as_ptr(t) as usize);
        let mut on_ground: Vec<[[f32; 2]; 3]> = Vec::new();
        let ground_lift =
            visioncraft_core::ground_model_lift(&project.shapes, project.terrain.is_some());
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
                            &|x, z| visioncraft_core::terrace::ground_at(t, &self.zones, x, z),
                        ));
                        self.model_ground.insert(m.id.clone(), (key, f.clone()));
                        f
                    }
                };
                let lift = if faces.is_empty() { 0.0 } else { ground_lift };
                if visible {
                    on_ground.extend(faces.iter());
                }
                // Hidden bodies keep their faces (so face indices and names stay) but lose their
                // triangles.
                let model = if m.hidden_bodies.is_empty() {
                    l.model.clone()
                } else {
                    let mut kept = (*l.model).clone();
                    let (mut triangles, mut faces) = (Vec::new(), Vec::new());
                    for b in &mut kept.bodies {
                        let start = triangles.len() as u32;
                        if !m.hides(b) {
                            let r = b.triangles.start as usize..b.triangles.end as usize;
                            triangles.extend_from_slice(&l.model.triangles[r.clone()]);
                            faces.extend_from_slice(&l.model.triangle_faces[r]);
                        }
                        b.triangles = start..triangles.len() as u32;
                    }
                    (kept.triangles, kept.triangle_faces) = (triangles, faces);
                    Arc::new(kept)
                };
                wanted.push((
                    m.id.clone(),
                    Geom::Model(l.version, m.hidden_bodies.clone()),
                    model,
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
        let zones_key = self
            .zones
            .iter()
            .fold(0usize, |k, z| k.rotate_left(11) ^ z.signature());
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
            // What the mesh depends on besides the shape: the terrain and the water on top, and
            // the terraces (what stands in one stands on its floor; a lawn is cut out of it).
            let terrace_area = matches!(&placed, Shape::Area(a) if a.terrace.is_some());
            let follows_ground = matches!(&placed, Shape::Extrusion(e)
                if e.stairs.is_some()
                    || e.fence.as_ref().is_some_and(|f| f.slope != visioncraft_core::fence::FenceSlope::Level));
            let depends = if draped {
                holes
                    .iter()
                    .flat_map(|h| h.iter())
                    .fold(terrain_key, |k, p| {
                        k.rotate_left(7)
                            ^ (p[0].to_bits() as usize)
                            ^ ((p[1].to_bits() as usize) << 1)
                    })
                    ^ zones_key
            } else if terrace_area {
                terrain_key
            } else if follows_ground {
                // Posts and steps stand on the ground under them: it changes with the terrain.
                terrain_key ^ zones_key
            } else {
                zones_key
            };
            let geom = Geom::Shape(Box::new(placed.clone()), depends);
            let visible = project.layer_visible(s.layer());
            // The mesh is only rebuilt when the shape (or the terrain it lies on) changed.
            let model = match self.owners.get(&id) {
                Some(o) if o.key.geom == geom => o.geom.clone(),
                _ => match match terrain.as_deref() {
                    // Cut along the terrain's own triangles: no hill edge pokes through.
                    Some(t) => shape_mesh_on_terrain_with(&placed, t, &holes, &self.zones),
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

        self.photo_wanted.clear();
        for (id, geom, model, xf, visible, is_shape, is_area) in wanted {
            live.push(id.clone());
            let mut owner_looks = |project: &Project| -> Vec<Look> {
                let resolved = resolve_materials(project, &id, &model);
                let forced = forced_look(project, &id);
                model
                    .faces
                    .iter()
                    .zip(resolved)
                    .map(|(f, m)| {
                        if clay {
                            CLAY
                        } else if let Some(l) = &forced {
                            *l
                        } else if let Some(i) = m {
                            look_of(
                                &project.materials[i],
                                &mut self.library,
                                u32::from(project.plant_month()),
                                &project.site,
                                project.view.weathering,
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
            // Context layers and clay show no photos, as they show no materials.
            let photos = if clay || forced_look(project, &id).is_some() {
                Vec::new()
            } else {
                self.photos_of(project, &id, &model, &xf)
            };
            let key = OwnerKey {
                geom,
                visible,
                looks: looks.iter().map(Look::key).collect(),
                photos,
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
                        self.build_pieces(r, &model, &looks, &key.photos, xf, &mut problems)
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

        let t = crate::assets::now_ms();
        self.sync_generators(r, project, assets);
        crate::load_stats::add(
            "~scene sync: generators (scatter, plants)",
            crate::assets::now_ms() - t,
        );
        let t = crate::assets::now_ms();
        self.sync_lawns(r, project);
        crate::load_stats::add("~scene sync: lawns", crate::assets::now_ms() - t);
        let t = crate::assets::now_ms();
        self.sync_clutter(r, project);
        crate::load_stats::add("~scene sync: clutter", crate::assets::now_ms() - t);
        let t = crate::assets::now_ms();
        self.sync_natural(r, project);
        crate::load_stats::add(
            "~scene sync: rocks, deadwood, cover",
            crate::assets::now_ms() - t,
        );
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
                            // Blades bend with the gusts far more than stems of wood.
                            r.set_item_grass(item, true);
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

    /// The renderer items of the lawns' tufts drawn (none when the GPU draws the grass).
    pub fn lawn_items(&self) -> usize {
        self.lawns.values().map(|l| l.items.len()).sum()
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

    /// Grass drawn on the GPU (`Quality::grass` above 0) or as tufts. True when it changed: the
    /// lawns are then dropped, for the next sync to make again the one way or the other.
    pub fn set_gpu_grass(&mut self, r: &mut Renderer, on: bool) -> bool {
        if self.gpu_grass == on {
            return false;
        }
        self.gpu_grass = on;
        self.drop_lawns(r);
        self.grass_key = None;
        r.set_grass(None);
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
        // Blades of their own colour (dune grass over sand): only the season changes them.
        if let SurfaceKind::Lawn3d(Lawn3d {
            blade_color: Some(c),
            ..
        }) = &m.kind
        {
            let t = lawn_season(month);
            return [0, 1, 2].map(|k| c[k] * t[k]);
        }
        // The meadow's photo is tinted to its own colour: its blades take that.
        let ground = if m.id == "lawn" && material_library::overlay(&self.library, m).is_some() {
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
        // Grass thins under the crowns of trees (as grown for the years shown): their circles,
        // a little inside the drip line.
        let crowns: Vec<[f32; 3]> = tree_crowns(project)
            .into_iter()
            .map(|(_, c)| [c[0], c[1], c[2] * 0.4])
            .collect();
        let todo = self.lawn_faces(project);
        if self.gpu_grass {
            self.drop_lawns(r);
            self.lawns_follow = false;
            self.sync_grass(r, project, &todo, &crowns);
            return;
        }
        let Some(res) = plants::resolve(project, LAWN_SPECIES) else {
            return;
        };
        let class = plants::class(&res.params, month);
        let (max_density, max_tufts, narrow) = if self.dense_lawns {
            (LAWN_MAX_DENSITY * 2.0, LAWN_MAX_TUFTS * 4, 0.7)
        } else {
            (LAWN_MAX_DENSITY, LAWN_MAX_TUFTS, 1.0)
        };
        let max_tufts = (max_tufts as f32 * self.lawn_budget.max(1.0)) as usize;
        self.lawns_follow = false;
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
            let over_blend = blends_over(&project.shapes, &id);
            over.extend(
                on_ground
                    .iter()
                    .map(|f| std::borrow::Cow::Borrowed(f.as_slice())),
            );
            let over_key: Vec<u32> = over
                .iter()
                .flat_map(|o| o.iter().flat_map(|p| [p[0].to_bits(), p[1].to_bits()]))
                .chain(over_blend.iter().map(|b| b.to_bits()))
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
                                l.stripes_deg.to_bits(),
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
                    .want(LAWN_SPECIES, None, class, month, &res.params, &[]);
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
            // shortcut: one colour per lawn (its first lawn face's), fine until lawns mix colours
            let color = faces[0].2;
            let job = LawnScatter {
                xf: o.xf.to_cols_array(),
                faces: faces.iter().map(|(f, l, _)| (*f, *l)).collect(),
                areas,
                max_density,
                max_tufts,
                narrow,
                window: window.map(|w| (w.center, w.radius)),
                over: over.iter().map(|a| a.to_vec()).collect(),
                over_blend,
                crowns: crowns.clone(),
            };
            let slot = start_lawn(o.geom.clone(), job);
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

    /// The visible lawn shapes and their lawn faces (face, its lawn turned into the drawing, its
    /// blades' colour), by shape id; none under deep snow, which buries the blades.
    fn lawn_faces(&self, project: &Project) -> Vec<(String, LawnFaces)> {
        if project.snow() > LAWN_SNOW_BURIES {
            return Vec::new();
        }
        let mut owners: Vec<&String> = self.owners.keys().collect();
        owners.sort();
        let mut todo = Vec::new();
        for id in owners {
            let o = &self.owners[id];
            if !o.key.visible || !o.is_shape {
                continue;
            }
            let faces: LawnFaces = resolve_materials(project, id, &o.geom)
                .into_iter()
                .enumerate()
                .filter_map(|(f, m)| {
                    let m = project.materials.get(m?)?;
                    match &m.kind {
                        SurfaceKind::Lawn3d(l) => Some((
                            f,
                            turned_lawn(l, &project.site),
                            self.blade_color(m, u32::from(project.plant_month())),
                        )),
                        _ => None,
                    }
                })
                .collect();
            if !faces.is_empty() {
                todo.push((id.clone(), faces));
            }
        }
        todo
    }

    /// The GPU grass's mask over every lawn face and the terrain's lawn and meadow layers
    /// (`visioncraft_core::grass`), given to the renderer when what it is made of changed: the
    /// lawns, the terrain, what lies on them, the crowns over them and the month; for a terrain
    /// too big for one mask also the eye's cell (`grass_eye`).
    fn sync_grass(
        &mut self,
        r: &mut Renderer,
        project: &Project,
        todo: &[(String, LawnFaces)],
        crowns: &[[f32; 3]],
    ) {
        let month = u32::from(project.plant_month());
        let on_ground = self.ground_faces.clone();
        // Each lawn with what lies on it (world x, z) and their soft edges.
        let lawns: Vec<(Holes, Vec<f32>)> = todo
            .iter()
            .map(|(id, _)| {
                let o = &self.owners[id];
                let to_world = |p: &[f32; 2]| {
                    let w = o.xf.transform_point3(Vec3::new(p[0], 0.0, p[1]));
                    [w.x, w.z]
                };
                let mut holes: Holes = areas_over(&project.shapes, id)
                    .iter()
                    .map(|a| a.iter().map(to_world).collect())
                    .collect();
                holes.extend(on_ground.iter().map(|t| t.to_vec()));
                (holes, blends_over(&project.shapes, id))
            })
            .collect();
        // The terrain's lawn and meadow layers (no mowing stripes: those belong to a drawn lawn),
        // without what is drawn over the terrain: areas, terraces, models on the ground.
        let terrain = self.terrain.clone();
        let layers: Vec<Option<(Lawn3d, [f32; 3])>> = terrain
            .as_ref()
            .map(|t| {
                t.layers
                    .iter()
                    .map(|l| {
                        let m = project.materials.iter().find(|m| m.id == l.material)?;
                        match &m.kind {
                            SurfaceKind::Lawn3d(lawn) => Some((
                                Lawn3d {
                                    stripes_m: 0.0,
                                    stripes_deg: 0.0,
                                    flowers: lawn.flowers * flower_season(month),
                                    ..*lawn
                                },
                                self.blade_color(m, month),
                            )),
                            _ => None,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let terrain_grass = layers.iter().any(Option::is_some);
        let mut terrain_holes: Holes = Vec::new();
        if terrain_grass {
            terrain_holes.extend(project.shapes.iter().filter_map(|s| match s {
                Shape::Area(a) if project.layer_visible(&a.layer) => Some(a.outline().into_owned()),
                _ => None,
            }));
            terrain_holes.extend(self.zones.iter().map(|z| z.hole.clone()));
            terrain_holes.extend(on_ground.iter().map(|t| t.to_vec()));
        }
        let min_height = project.sea.as_ref().map(|s| s.level + 0.05);
        let windowed = terrain_grass
            && terrain.as_ref().is_some_and(|t| {
                let [lo, hi] = t.extent();
                hi[0] - lo[0] > grass::WINDOW_M || hi[1] - lo[1] > grass::WINDOW_M
            });
        self.grass_follow = windowed;
        let window = windowed.then(|| grass::window_round(self.grass_eye));

        let mut bits: Vec<u32> = vec![month];
        bits.extend(crowns.iter().flat_map(|c| c.map(f32::to_bits)));
        let mut geoms = Vec::new();
        for ((id, faces), (holes, blends)) in todo.iter().zip(&lawns) {
            let o = &self.owners[id];
            geoms.push(o.key.geom.clone());
            bits.extend(o.xf.to_cols_array().map(f32::to_bits));
            for (f, l, c) in faces {
                bits.push(*f as u32);
                bits.extend(lawn_bits(l));
                bits.extend(c.map(f32::to_bits));
            }
            bits.extend(holes.iter().flatten().flat_map(|p| p.map(f32::to_bits)));
            bits.extend(blends.iter().map(|b| b.to_bits()));
        }
        if terrain_grass && let Some(t) = &terrain {
            // The terrain is replaced (a new `Arc`) whenever it changes.
            let at = Arc::as_ptr(t) as usize as u64;
            bits.extend([at as u32, (at >> 32) as u32]);
            for l in &layers {
                match l {
                    Some((lawn, c)) => {
                        bits.push(1);
                        bits.extend(lawn_bits(lawn));
                        bits.extend(c.map(f32::to_bits));
                    }
                    None => bits.push(0),
                }
            }
            bits.extend(
                terrain_holes
                    .iter()
                    .flatten()
                    .flat_map(|p| p.map(f32::to_bits)),
            );
            bits.extend(min_height.map(f32::to_bits));
            bits.extend(window.iter().flatten().flat_map(|p| p.map(f32::to_bits)));
        }
        let key = (geoms, bits);
        if self.grass_key.as_ref() == Some(&key) {
            return;
        }
        let mut kinds: Vec<visioncraft_render::GrassKind> = Vec::new();
        let mut kind_of = |kind: visioncraft_render::GrassKind| -> u8 {
            match kinds.iter().position(|x| *x == kind) {
                Some(k) => k as u8,
                None if kinds.len() < visioncraft_render::GRASS_KINDS => {
                    kinds.push(kind);
                    (kinds.len() - 1) as u8
                }
                // shortcut: past eight lawn looks the rest take the first's colour
                None => 0,
            }
        };
        let mut patches = Vec::new();
        for ((id, faces), (holes, blends)) in todo.iter().zip(&lawns) {
            let o = &self.owners[id];
            for (f, l, color) in faces {
                let a = l.stripes_deg.to_radians();
                let k = kind_of(visioncraft_render::GrassKind {
                    color: *color,
                    stripes_m: l.stripes_m,
                    stripes_dir: [a.sin(), -a.cos()],
                    dry: grass_dry(month, l.blade_height_m),
                });
                patches.push(grass::GrassPatch {
                    model: &o.geom,
                    face: *f,
                    xf: o.xf.to_cols_array(),
                    lawn: Lawn3d {
                        flowers: l.flowers * flower_season(month),
                        ..*l
                    },
                    kind: k,
                    holes,
                    hole_blends: blends,
                });
            }
        }
        let terrain_layers: Vec<Option<(Lawn3d, u8)>> = layers
            .iter()
            .map(|l| {
                l.map(|(lawn, color)| {
                    let k = kind_of(visioncraft_render::GrassKind {
                        color,
                        stripes_m: 0.0,
                        stripes_dir: [0.0, -1.0],
                        dry: grass_dry(month, lawn.blade_height_m),
                    });
                    (lawn, k)
                })
            })
            .collect();
        let grown = terrain
            .as_ref()
            .filter(|_| terrain_grass)
            .map(|t| grass::TerrainGrass {
                terrain: t,
                layers: terrain_layers,
                holes: &terrain_holes,
                min_height,
            });
        let mask = grass::grass_mask_in(&patches, grown.as_ref(), crowns, window);
        match &mask {
            Some(m) => r.set_grass(Some(&visioncraft_render::GrassField {
                origin: m.origin,
                cell_m: m.cell_m,
                size: m.size,
                params: &m.params,
                heights: &m.heights,
                kinds: &kinds,
                flowers: flower_colors(month),
            })),
            None => r.set_grass(None),
        }
        self.grass_key = Some(key);
    }

    fn build_pieces(
        &mut self,
        r: &mut Renderer,
        model: &ImportedModel,
        looks: &[Look],
        photos: &[u32],
        xf: Mat4,
        problems: &mut Vec<String>,
    ) -> (Vec<Piece>, Vec<Option<usize>>) {
        // Triangles grouped by look, in first-seen order; those in an area's soft edge band
        // (`ImportedModel::edge` below 1 at a corner) in groups of their own, whose material
        // dithers (scene.wgsl `edge_coverage`) and which carry the edge value in their UVs: only
        // the band pays for the cutout pass. A face with a wall photo is cut along the photo's
        // region: the part inside is a group of its own with UVs from the face's frame, the rest
        // keeps the face's look.
        let mut groups: Groups = Vec::new();
        let mut by_key: HashMap<(LookKey, bool), usize> = HashMap::new();
        let mut face_group: Vec<Option<usize>> = vec![None; model.faces.len()];
        for (t, tri) in model.triangles.iter().enumerate() {
            let face = model.triangle_faces[t] as usize;
            let look = looks[face];
            let soft = tri
                .iter()
                .any(|&v| model.edge.get(v as usize).is_some_and(|e| *e < 1.0));
            let photo = photos.get(face).copied().unwrap_or(0);
            let corners = tri.map(|v| Corner {
                position: model.positions[v as usize],
                normal: model.normals[v as usize],
            });
            if photo != 0
                && !soft
                && let Some(pf) = self
                    .photo_faces
                    .get(photo as usize - 1)
                    .copied()
                    .filter(|pf| pf.frame.faces(corners.map(|c| c.position)))
            {
                let (inside, outside) = split_by_region(&pf.frame, &pf.region, corners);
                let (w, h) = (pf.region.width(), pf.region.height());
                let uv = |c: &Corner| {
                    let [u, v] = pf.frame.uv(c.position);
                    [(u - pf.region.u0) / w, 1.0 - (v - pf.region.v0) / h]
                };
                for part in inside {
                    let uvs = part.map(|c| uv(&c));
                    push_triangle(
                        &mut groups,
                        &mut by_key,
                        &mut face_group,
                        face,
                        Look::of_photo(photo),
                        false,
                        part,
                        Some(uvs),
                    );
                }
                for part in outside {
                    push_triangle(
                        &mut groups,
                        &mut by_key,
                        &mut face_group,
                        face,
                        look,
                        false,
                        part,
                        None,
                    );
                }
                continue;
            }
            let edge = soft.then(|| tri.map(|v| [model.edge[v as usize], 1.0]));
            push_triangle(
                &mut groups,
                &mut by_key,
                &mut face_group,
                face,
                look,
                soft,
                corners,
                edge,
            );
        }
        let mut pieces = Vec::new();
        let mut group_piece = vec![None; groups.len()];
        for (g, (look, soft, data, uvs)) in groups.iter().enumerate() {
            // A group with UVs (a soft edge band, a photo) has them for every vertex.
            let has_uvs = *soft || look.photo != 0;
            let Some(mesh) = upload_uvs(r, data, true, has_uvs.then_some(uvs.as_slice())) else {
                problems.push("a mesh was too large for the GPU and is not drawn".into());
                continue;
            };
            let mat = if *soft {
                self.soft_material(r, look)
            } else {
                self.material(r, look)
            };
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

    /// The wall photos of owner `id` that can be drawn now, per face: 1 + the index in
    /// `photo_faces`, 0 for a face without one (or whose straightened picture has not arrived;
    /// it is asked for, and `photos_changed` says when it is there). Empty when no face has one.
    fn photos_of(
        &mut self,
        project: &Project,
        id: &str,
        model: &ImportedModel,
        xf: &Mat4,
    ) -> Vec<u32> {
        let mut out = Vec::new();
        for w in project
            .wall_photos
            .iter()
            .filter(|w| w.target.owner() == id)
        {
            let visioncraft_core::Target::Face { face, .. } = &w.target else {
                continue;
            };
            let Some(f) = model.faces.iter().position(|x| x.name == *face) else {
                continue;
            };
            let up = xf.inverse().transform_vector3(Vec3::Y).to_array();
            let Ok(frame) = face_frame(model, f, up) else {
                continue;
            };
            let region = w.region.unwrap_or_else(|| frame.whole());
            let Some(len) = self.library.store.provided(&w.file).map(|b| b.len()) else {
                continue;
            };
            let size = rectified_size(region.width(), region.height(), w.size_px);
            let path = crate::assets::photo_path(&w.file, len, w.size_px, w.corners_px, size);
            let texture = self.library.store.texture(&path, true, false, 0.0);
            self.photo_wanted.push((path, texture));
            let Some(texture) = texture else { continue };
            let pf = PhotoFace {
                texture,
                frame,
                region,
            };
            let at = match self.photo_faces.iter().position(|p| *p == pf) {
                Some(i) => i,
                None => {
                    self.photo_faces.push(pf);
                    self.photo_faces.len() - 1
                }
            };
            if out.is_empty() {
                out = vec![0; model.faces.len()];
            }
            out[f] = at as u32 + 1;
        }
        out
    }

    /// A photo's straightened picture arrived (or left the store) since the last sync: the scene
    /// has to be synced again. Called every frame.
    pub fn photos_changed(&mut self) -> bool {
        let store = &mut self.library.store;
        self.photo_wanted
            .iter()
            .any(|(path, had)| store.texture(path, true, false, 0.0) != *had)
    }

    /// The frame of a face of owner `owner` in the owner's own space, and its size in metres:
    /// what a region on it is measured in. A message for a face that is not there or not flat.
    pub fn face_frame_of(
        &self,
        owner: &str,
        face: &str,
    ) -> Result<visioncraft_core::wall_photo::FaceFrame, String> {
        let o = self
            .owners
            .get(owner)
            .ok_or_else(|| format!("no `{owner}` in the scene"))?;
        let f = o
            .geom
            .faces
            .iter()
            .position(|x| x.name == face)
            .ok_or_else(|| format!("`{owner}` has no face `{face}`"))?;
        face_frame(
            &o.geom,
            f,
            o.xf.inverse().transform_vector3(Vec3::Y).to_array(),
        )
    }

    // ---- planting ----

    /// The terrain's height at `(x, z)`, or a terrace's floor inside one (`terrace::ground_at`).
    pub fn terrain_at(&self, x: f32, z: f32) -> Option<f32> {
        visioncraft_core::terrace::ground_at(self.terrain.as_deref(), &self.zones, x, z)
    }

    /// The first hit of a ray with the terrain as it is drawn: a terrace's footprint is cut out.
    pub fn terrain_raycast(&self, origin: Vec3, dir: Vec3) -> Option<(f32, [f32; 3], [f32; 3])> {
        visioncraft_core::terrace::terrain_raycast(
            self.terrain.as_deref()?,
            &self.zones,
            origin.to_array(),
            dir.to_array(),
        )
    }

    /// The ground plants stand on: the terrain, areas and imported models, the highest wins.
    pub fn ground_fn(&self) -> impl Fn(f32, f32) -> Option<f32> + '_ {
        // Planting stands on areas and imported models, not on the roof of a house block.
        move |x, z| {
            let terrain = self.terrain_at(x, z);
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
                    self.upgrade(r, project, assets, month, g.id(), &mut o.groups);
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
                    .want(asset, res.scan_id.as_deref(), class, month, &params, &[]);
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
            let last = parts.iter().map(|p| p.2).fold(0.0, f32::max);
            // A plant with its impostor (MEDIUM and LOW): its coarsest model hands over to it
            // once the plant is small on screen (`PLANT_IMPOSTOR_M_PER_M`).
            if let Some((_, h)) = self.library.plant_impostor(asset) {
                let end = (PLANT_IMPOSTOR_M_PER_M * h).max(parts_before(&parts, last) + 1.0);
                for p in parts.iter_mut().filter(|p| p.2 == last) {
                    p.2 = end;
                }
                return Some(parts);
            }
            // Small objects (a pot, a bench) stop drawing far away, at a size-proportional distance.
            let size = self
                .library
                .catalog
                .as_ref()
                .and_then(|c| c.asset(asset))
                .map_or(1.0, |a| a.size_m.into_iter().fold(0.0, f32::max));
            if size < SMALL_OBJECT_M {
                let cull = (OBJECT_CULL_PER_M * size).clamp(60.0, 1500.0);
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
            // A catalogue model's level distances are for its size in the file: a pot drawn at
            // 2.4 times that size keeps each level 2.4 times as far.
            let scaled;
            let parts = if self.library.knows(asset) {
                scaled = scale_lod_distances(parts, matrices);
                &scaled
            } else {
                parts
            };
            let item = r.add_lod_item(parts, matrices);
            // Library models stop at the quality's model draw distance (`Library::caps_draw`).
            r.set_item_draw_capped(item, self.library.caps_draw(asset));
            // A hedge's shadow is a band, not one blob per segment.
            let clipped = asset
                .strip_prefix("flora:")
                .and_then(flora::Species::by_id)
                .is_some_and(|s| matches!(s.params.form, flora::Form::Clipped(_)));
            r.set_item_shadow_exact(item, clipped);
            // Conifers sway stiffer than leafed broadleaves (wind.rs `plant_hz`).
            let conifer = asset
                .strip_prefix("flora:")
                .and_then(flora::Species::by_id)
                .is_some_and(|s| matches!(s.params.form, flora::Form::Conifer(_)));
            r.set_item_conifer(item, conifer);
            let impostor = self.impostor_for(project, asset, variant, when);
            let last = parts.iter().map(|p| p.2).fold(0.0, f32::max);
            match impostor {
                // fails only for a removed impostor: the tree then stops at its far mesh
                Some((imp, max, small)) => {
                    if r.set_item_impostor(item, imp, max).is_ok() {
                        // A tree's shadow in the levels past its finest: a few blobs, no cards.
                        if let Some(m) = self.shadow_for(project, asset, variant, when) {
                            r.set_item_shadow_caster(item, m);
                        }
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
                // Where it stands decides how soon it is made (the nearest first).
                let at: Vec<Vec3> = matrices
                    .iter()
                    .step_by(matrices.len().div_ceil(64).max(1))
                    .map(|m| m.w_axis.truncate())
                    .collect();
                self.flora.want(
                    asset,
                    res.scan_id.as_deref(),
                    class,
                    when.month,
                    &params,
                    &at,
                );
                self.proto(r, asset, || placeholder::flora_parts(asset, &res))
            }
            // A catalogue model still loading, or a plain asset: its box from the project assets.
            None => {
                let known = assets.iter().find(|a| a.id == asset);
                // Once the catalogue is here, an id it lacks (a saved file's) stays a cube.
                if known.is_none()
                    && self.library.catalog.is_some()
                    && !self.protos.contains_key(asset)
                {
                    let why = format!("unknown asset `{asset}`: drawn as a placeholder cube");
                    #[cfg(target_arch = "wasm32")]
                    log::warn!("{why}");
                    #[cfg(not(target_arch = "wasm32"))]
                    eprintln!("warning: {why}");
                }
                self.proto(r, asset, || placeholder::parts(known))
            }
        };
        protos
            .into_iter()
            .map(|(mesh, mat)| r.add_item(mesh, mat, matrices))
            .collect()
    }

    /// A generated tree's shadow caster (`Flora::shadow_caster`).
    fn shadow_for(
        &self,
        project: &Project,
        asset: &str,
        variant: u32,
        when: When,
    ) -> Option<MeshId> {
        if self.library.knows(asset) {
            return None;
        }
        let res = plants::resolve(project, asset)?;
        let (class, _) = when.class(&res.params);
        self.flora.shadow_caster(asset, class, variant)
    }

    /// A plant's baked impostor and how far it draws (catalogue models: a plant's on MEDIUM and
    /// LOW, `Library::set_plant_impostors`).
    fn impostor_for(
        &self,
        project: &Project,
        asset: &str,
        variant: u32,
        when: When,
    ) -> Option<(ImpostorId, f32, bool)> {
        if self.library.knows(asset) {
            return self
                .library
                .plant_impostor(asset)
                .map(|(imp, h)| (imp, plants::impostor_max(h), h < plants::TREE_M));
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
        generator: &str,
        groups: &mut HashMap<(String, u32), Group>,
    ) {
        for ((asset, variant), grp) in groups.iter_mut() {
            let when = When {
                month,
                age: grp.age,
            };
            // Built before the catalogue arrived, the group's model was never asked for.
            if self.library.knows(asset) {
                let capped = self.library.caps_draw(asset);
                self.library
                    .note(generator, asset, *variant, &grp.matrices, capped);
            }
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
                let capped = self.library.caps_draw(&asset);
                self.library
                    .note(g.id(), &asset, variant, &matrices, capped);
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
        let site = project.site;
        self.terrain_gpu
            .sync_layers(r, project, &mut self.library, &mut |m, lib| {
                // Mowing stripes belong to the drawn lawn, not the ground round it.
                // The ground is not aged here (terrain.wgsl shades it without).
                Look {
                    stripes: 0,
                    aging: [0.0; 4],
                    ..look_of(m, lib, month, &site, 0.0)
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
                aging: [0.0; 4],
                ..look_of(m, &mut self.library, month, &project.site, 0.0)
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
        let cover = [eye.x, eye.z].map(|v| (v / natural::COVER_CELL_M).floor() as i32);
        let cover_moved =
            std::mem::replace(&mut self.natural.eye, cover) != cover && self.natural.follow;
        let grass_cell = grass::window_cell([eye.x, eye.z]);
        let grass_moved =
            std::mem::replace(&mut self.grass_eye, grass_cell) != grass_cell && self.grass_follow;
        self.flora.pump(r, eye) | library | moved | cover_moved | grass_moved
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
            _ => {
                let mut e = evaluate_in_scene(
                    g,
                    &self.ground_fn(),
                    &|x, z| self.open_ground(x, z),
                    &|p| self.up_to_building(p),
                    &|a| self.plant_dims(a),
                );
                let catalog = self.library.catalog.as_ref();
                visioncraft_core::keep_dry(self.sea.as_ref(), &mut e, &|a| {
                    catalog
                        .and_then(|c| c.asset(a))
                        .is_some_and(visioncraft_core::water::floats)
                });
                e
            }
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
        let terrain = self.terrain_at(x, z);
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
        parts: impl FnOnce() -> Vec<(MeshData, Look)>,
    ) -> Vec<(MeshId, MaterialId)> {
        if let Some(p) = self.protos.get(id) {
            return p.clone();
        }
        let mut out = Vec::new();
        for (data, look) in parts() {
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
        let terrain = self.terrain_at(x, z).unwrap_or(ground);
        if self
            .raycast(Vec3::new(x, terrain + 200.0, z), Vec3::NEG_Y, false)
            .is_some_and(|p| p.point.y > terrain + 0.3)
        {
            return false;
        }
        let water = project.shapes.iter().any(|s| {
            let Shape::Area(a) = s else { return false };
            inside_outline([x, z], &a.outline())
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

    /// The world triangles that block the sky for the sky visibility volume: the visible
    /// imported models' faces and the drawn houses' (blocks with a roof, windows or doors), less
    /// glass. Other shapes and objects are left out: a garden's walls and furniture are the
    /// screen-space occlusion's to darken.
    pub fn sky_occluders(&self, project: &Project) -> Vec<[[f32; 3]; 3]> {
        let mut ids: Vec<&String> = self.owners.keys().collect();
        ids.sort();
        let mut out = Vec::new();
        for id in ids {
            let o = &self.owners[id];
            let house = || {
                project.shapes.iter().any(|s| {
                    matches!(s, Shape::Extrusion(e) if &e.id == id && e.closed && e.house.is_some())
                })
            };
            if !o.key.visible || o.is_area || (o.is_shape && !house()) {
                continue;
            }
            let materials = resolve_materials(project, id, &o.geom);
            let clear = |face: u32| {
                materials
                    .get(face as usize)
                    .copied()
                    .flatten()
                    .and_then(|m| project.materials.get(m))
                    .is_some_and(|m| {
                        matches!(m.kind, SurfaceKind::ArchitecturalGlass { .. })
                            || m.base_color[3] < 0.99
                    })
            };
            for (t, face) in o.geom.triangles.iter().zip(&o.geom.triangle_faces) {
                if clear(*face)
                    || o.face_piece
                        .get(*face as usize)
                        .is_some_and(Option::is_none)
                {
                    continue;
                }
                out.push(t.map(|v| {
                    o.xf.transform_point3(Vec3::from(o.geom.positions[v as usize]))
                        .to_array()
                }));
            }
        }
        out
    }

    /// The crowns of the trees that dim the sky (`visioncraft_core::sky_volume`): every planted
    /// plant at least `CANOPY_MIN_HEIGHT_M` tall, as an ellipsoid over its upper three quarters.
    // shortcut: any plant over 3 m counts as a leafy crown, in every season; the leaf-off winter
    // and the conifers are not told apart.
    pub fn sky_crowns(&self) -> Vec<visioncraft_core::Crown> {
        const CANOPY_MIN_HEIGHT_M: f32 = 3.0;
        let mut ids: Vec<&String> = self.gens.keys().collect();
        ids.sort();
        ids.into_iter()
            .filter(|id| !self.gens[*id].hidden)
            .flat_map(|id| self.gens[id].insts.iter())
            .filter(|i| i.height >= CANOPY_MIN_HEIGHT_M && i.radius > 0.5)
            .map(|i| visioncraft_core::Crown {
                center: [i.pos[0], i.pos[1] + i.height * 0.625, i.pos[2]],
                radii: [i.radius, i.height * 0.375, i.radius],
                ground_y: i.pos[1],
            })
            .collect()
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
        let hit = self
            .raycast(Vec3::new(x, from_y, z), Vec3::NEG_Y, false)
            .map(|h| h.point.y);
        // The terrain (a terrace's floor inside one) is a floor too, when it is below the cast.
        let ground = self.terrain_at(x, z).filter(|g| *g <= from_y);
        match (hit, ground) {
            (Some(h), Some(g)) => Some(h.max(g)),
            (h, g) => h.or(g),
        }
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
            geom: Geom::Model(version, Vec::new()),
            visible,
            looks,
            photos: Vec::new(),
        }
    }

    #[test]
    fn classify_picks_the_cheapest_update() {
        let a = key(1, vec![[0; 23]], true);
        let (i, t) = (Mat4::IDENTITY, Mat4::from_translation(Vec3::X));
        assert_eq!(classify(&a, &a.clone(), &i, &i), Change::Keep);
        assert_eq!(classify(&a, &a.clone(), &i, &t), Change::Transform);
        assert_eq!(
            classify(&a, &key(2, vec![[0; 23]], true), &i, &i),
            Change::Rebuild
        );
        assert_eq!(
            classify(&a, &key(1, vec![[1; 23]], true), &i, &i),
            Change::Rebuild
        );
        assert_eq!(
            classify(&a, &key(1, vec![[0; 23]], false), &i, &t),
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
            hidden_bodies: Vec::new(),
            extensions: Default::default(),
        };
        let p = model_matrix(&m).transform_point3(Vec3::X);
        assert!((p - Vec3::new(1.0, 0.0, -2.0)).length() < 1e-5, "{p}");
    }

    #[test]
    fn context_owners_get_the_massing_look_whatever_their_materials() {
        use serde_json::json;
        let mut s = visioncraft_core::Session::new(Project::default());
        let r = s
            .run(
                "layer.add",
                json!({"name": "Surroundings", "context": true}),
            )
            .unwrap();
        let l = r["id"].as_str().unwrap().to_string();
        let outline = json!([[0, 0], [4, 0], [4, 4], [0, 4]]);
        s.run(
            "shape.add_extrusion",
            json!({"name": "Neighbour", "outline": outline, "height": 6, "material": "brick", "layer": l}),
        )
        .unwrap();
        s.run(
            "shape.add_extrusion",
            json!({"name": "Mine", "outline": outline, "height": 3, "material": "brick"}),
        )
        .unwrap();
        let (neighbour, mine) = (
            s.project().shapes[0].id().to_string(),
            s.project().shapes[1].id().to_string(),
        );
        assert_eq!(
            forced_look(s.project(), &neighbour).map(|l| l.key()),
            Some(MASSING.key())
        );
        assert!(forced_look(s.project(), &mine).is_none());
        assert_ne!(MASSING.key(), CLAY.key());
    }

    #[test]
    fn equal_looks_share_a_key() {
        assert_eq!(CLAY.key(), CLAY.key());
        assert_ne!(CLAY.key(), UNASSIGNED.key());
    }
}
