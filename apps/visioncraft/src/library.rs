//! The asset library as the scene uses it: the three indexes, catalogue models streamed in as
//! multi-part LOD items, the textures of library materials, and the lamps' emitters.
//!
//! Everything arrives through the `AssetStore`; this file turns what arrived into renderer meshes
//! and materials, within the frame's upload budget, and decides what to ask for next from where
//! the camera is. While a model is not resident the caller keeps its placeholder.

use crate::assets::{
    AssetStore, Budget, GlbData, Key, Loaded, Rgba, State, now_ms, priority, texture_cost,
};
use crate::gltf_load::{Alpha, Glb, GlbMaterial};
use glam::{Mat4, Vec3};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use visioncraft_core::Catalog;
use visioncraft_core::catalog::Lod;
use visioncraft_render::{
    AlphaMode, ColorSpace, ImpostorBake, ImpostorId, Material, MaterialId, Mesh, MeshId,
    Projection, Renderer, Surface, TextureDesc, TextureId,
};

// ---- indexes ----

#[derive(Clone, Debug, Default, Deserialize)]
pub struct LibMaps {
    #[serde(default)]
    pub basecolor: Option<String>,
    #[serde(default)]
    pub normal: Option<String>,
    #[serde(default)]
    pub orm: Option<String>,
    #[serde(default)]
    pub height: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LibMaterial {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub source_url: String,
    #[serde(default)]
    pub size_m: Option<[f32; 2]>,
    #[serde(default)]
    pub maps: LibMaps,
    #[serde(default)]
    pub thumbnail: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LibSky {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub source_url: String,
    pub file: String,
    #[serde(default)]
    pub thumbnail: String,
}

#[derive(Deserialize)]
struct MaterialsFile {
    materials: Vec<LibMaterial>,
}

#[derive(Deserialize)]
struct SkiesFile {
    skies: Vec<LibSky>,
}

// ---- textured project materials ----

/// Where one map of a project material comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TexSrc {
    /// A library-relative path.
    Lib(String),
    /// Decoded pixels the app holds (a dropped `.mcmat`), by name.
    Mem(String),
}

/// The maps of a project material. `orm` is the packed occlusion-roughness-metallic map.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TexSpec {
    pub base: Option<TexSrc>,
    pub normal: Option<TexSrc>,
    pub orm: Option<TexSrc>,
    pub roughness: Option<TexSrc>,
    pub metallic: Option<TexSrc>,
    pub ao: Option<TexSrc>,
    /// Height (white = top), for parallax; shares the renderer's occlusion slot, so it is
    /// bound only when `ao` is absent.
    pub height: Option<TexSrc>,
}

impl TexSpec {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A material built for a spec: kept so it can be completed when its textures arrive.
struct Textured {
    material: MaterialId,
    base: Material,
    spec: usize,
}

// ---- models ----

struct Model {
    parts: Vec<(MeshId, MaterialId)>,
    /// Emitting materials with their template (strength 0), for the lamps toggle.
    emitters: Vec<(MaterialId, Material)>,
    lumens: f32,
    textures: Vec<TextureId>,
    meshes: Vec<MeshId>,
    materials: Vec<MaterialId>,
}

struct Upload {
    path: String,
    data: GlbData,
    /// (image index, sRGB) of every texture this model needs, deduplicated.
    slots: Vec<(usize, bool)>,
    ids: Vec<TextureId>,
    lumens: f32,
}

/// Luminance (cd/m2) of a fixture's emitter for a lamp of `lumens`, taking its glowing surface as
/// about 0.02 m2 (Lambertian: L = flux / (pi * area)).
pub fn emitter_luminance(lumens: f32) -> f32 {
    (lumens / (std::f32::consts::PI * 0.02)).clamp(500.0, 30_000.0)
}

fn glb_slots(glb: &Glb) -> Vec<(usize, bool)> {
    let used: HashSet<usize> = glb.prims.iter().map(|p| p.material).collect();
    let mut slots = Vec::new();
    for m in glb
        .materials
        .iter()
        .enumerate()
        .filter(|(i, m)| used.contains(i) && !m.glass)
        .map(|(_, m)| m)
    {
        for s in [
            m.base_image.map(|b| (b, true)),
            m.normal_image.map(|n| (n, false)),
            m.orm_image.map(|o| (o, false)),
        ]
        .into_iter()
        .flatten()
        {
            if !slots.contains(&s) {
                slots.push(s);
            }
        }
    }
    slots
}

fn model_material(
    gm: &GlbMaterial,
    tex: &dyn Fn(Option<usize>, bool) -> Option<TextureId>,
) -> Material {
    let mut m = Material {
        base_color: gm.base_color,
        metallic: gm.metallic,
        roughness: gm.roughness,
        projection: Projection::Uv,
        double_sided: gm.double_sided,
        ..Default::default()
    };
    if gm.glass {
        // The renderer's glass: `base_color.rgb` is the tint, alpha the opacity.
        let opacity = if gm.alpha == Alpha::Blend {
            gm.base_color[3]
        } else {
            0.12
        };
        m.base_color[3] = opacity.clamp(0.05, 0.6);
        m.surface = Surface::Glass;
        m.metallic = 0.0;
        m.roughness = gm.roughness.min(0.15);
        return m;
    }
    m.base_color_map = tex(gm.base_image, true);
    m.normal_map = tex(gm.normal_image, false);
    m.orm_map = tex(gm.orm_image, false);
    m.emissive = gm.emissive;
    m.emissive_strength = gm.emissive_strength;
    match gm.alpha {
        Alpha::Opaque => {}
        Alpha::Mask(t) => {
            m.alpha_mode = AlphaMode::Cutout { threshold: t };
            m.foliage = gm.double_sided;
        }
        Alpha::Blend => m.alpha_mode = AlphaMode::Blend,
    }
    m
}

pub struct Library {
    pub store: AssetStore,
    pub catalog: Option<Arc<Catalog>>,
    pub materials: Vec<LibMaterial>,
    pub skies: Vec<LibSky>,
    /// The higher tiers of library files (`tiers.json`), when the library has them.
    pub tiers: Option<Arc<visioncraft_core::Tiers>>,
    /// The texture tier materials use when the files are there: "1k" (the base files), "2k" or
    /// "4k" (`Studio::prepare` sets it from the quality or the Textures choice).
    pub texture_tier: &'static str,
    /// Pixels per texture repeat by texture spec (`set_need`).
    need: HashMap<usize, f32>,
    /// Tier files asked for and not yet resident, with their expected bytes.
    tier_asked: std::cell::RefCell<HashMap<String, u64>>,
    /// Tier models that did not fit when they arrived; asked again after a tier change.
    tier_refused: HashSet<String>,
    /// Set when the catalogue has just arrived (the studio adds its assets for placeholders).
    pub catalog_arrived: bool,
    /// The project's imported objects as catalogue assets (`user_objects`).
    pub(crate) user_assets: Vec<visioncraft_core::catalog::Asset>,
    /// Metres each imported model file is raised by so its lowest point is on the ground.
    pub ground_offsets: HashMap<String, f32>,
    indexes_asked: bool,
    models: HashMap<String, Model>,
    awaiting: HashSet<String>,
    uploading: Vec<Upload>,
    garbage: Vec<Model>,
    /// Where each generator's library assets stand, to see how near the camera they are.
    wants: HashMap<String, Vec<Noted>>,
    /// Per placed catalogue asset at the last scan: its nearest instance's distance (m) and the
    /// level of detail it needs there (`lod_report`; the level count when it is past the model
    /// draw distance).
    lod_seen: HashMap<String, (f32, usize)>,
    /// A want was noted since the last scan: its model may not be asked for yet.
    unscanned: bool,
    pub lamps_on: bool,
    lamps_applied: Option<bool>,
    /// Bumps whenever something a scene item is built from changed.
    pub rev: u64,
    pub(crate) specs: Vec<TexSpec>,
    textured: Vec<Textured>,
    mem: HashMap<String, Rgba>,
    mem_tex: HashMap<String, TextureId>,
    resolved_store_rev: u64,
    dirty: bool,
    pub messages: Vec<String>,
    pub worst_pump_ms: f64,
    /// Impostors of catalogue plants (MEDIUM and LOW, `set_plant_impostors`): a plant's coarsest
    /// model hands over to one, baked from the model before it. The scanned shrubs' coarsest
    /// models keep thousands of triangles each (some no fewer than the level before), and a
    /// coastal planting draws hundreds of them far off, in every pass.
    plant_impostors: bool,
    impostors: HashMap<String, ImpostorId>,
    /// Plants whose bake failed: they stay meshes.
    no_impostor: HashSet<String>,
    baking: Option<(String, ImpostorBake)>,
}

impl Default for Library {
    fn default() -> Self {
        Self::new()
    }
}

/// One (asset, variant) group of a generator as the scene draws it: where its instances stand,
/// the scale its level distances are multiplied by (`scene::scale_lod_distances`: the group's
/// largest) and whether it stops at the model draw distance.
struct Noted {
    asset: String,
    variant: u32,
    at: Vec<Vec3>,
    scale: f32,
    capped: bool,
}

/// What a scan sees from: the eye, the renderer's level-of-detail scale (`Renderer::lod_scale`),
/// the model draw distance (`Renderer::model_draw_distance`) and the camera (world to clip;
/// `None`: everything counts as in view).
#[derive(Clone, Copy, Debug)]
pub struct ScanView {
    pub eye: Vec3,
    pub lod_scale: f32,
    pub draw_m: f32,
    pub view_proj: Option<Mat4>,
}

impl ScanView {
    pub fn from_renderer(r: &Renderer, eye: Vec3) -> Self {
        Self {
            eye,
            lod_scale: r.lod_scale(),
            draw_m: r.model_draw_distance(),
            view_proj: Some(r.view_proj()),
        }
    }
}

/// How much nearer than an instance the renderer may measure it: it takes a level per cluster
/// of instances (8 m cells) from the cluster's nearest point, so an instance can show a level
/// finer than its own distance says.
const CLUSTER_SLACK_M: f32 = 12.0;

/// Metres added to the priority of a level wanted only by instances out of view: those in view
/// load first.
const OUT_OF_VIEW_M: f32 = 40.0;

/// Where each level of a group ends, metres from the eye, as the renderer compares them: the
/// catalogue's distances times the group's scale and the LOD scale, cut at the draw distance
/// for a capped group (`lod::cap`).
fn level_ends(lods: &[Lod], scale: f32, view: &ScanView, capped: bool) -> Vec<f32> {
    let mut ends: Vec<f32> = lods
        .iter()
        .map(|l| (l.max_distance_m * scale * view.lod_scale).min(f32::MAX))
        .collect();
    if capped {
        visioncraft_render::lod::cap(&mut ends, view.draw_m);
    }
    ends
}

/// The level (slot) for `d` metres; `ends.len()` past the last.
fn level_at(ends: &[f32], d: f32) -> usize {
    ends.iter().position(|&e| d <= e).unwrap_or(ends.len())
}

/// The resident model a level (slot) draws, as `parts` picks it: its own, else the next coarser
/// resident one, else a finer one.
fn shown_model(slot: usize, resident: &[bool]) -> Option<usize> {
    (slot..resident.len())
        .find(|&j| resident[j])
        .or_else(|| (0..slot.min(resident.len())).rev().find(|&j| resident[j]))
}

/// Whether a sphere at `p` of radius `r` is inside the four side planes of `view_proj`'s
/// frustum (near and far left out: an infinite far plane has none).
fn in_view(view_proj: Mat4, p: Vec3, r: f32) -> bool {
    let (r0, r1, r3) = (view_proj.row(0), view_proj.row(1), view_proj.row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1].iter().all(|pl| {
        let n = pl.truncate();
        let len = n.length();
        len <= 0.0 || (n.dot(p) + pl.w) / len >= -r
    })
}

/// What one scan concludes for an asset's levels from all its instances.
#[derive(Default, Debug)]
struct LevelPlan {
    /// The nearest instance's distance (m) and the level it needs (the level count past the
    /// draw distance).
    nearest: Option<(f32, usize)>,
    /// Levels some instance within the draw distance shows, with the most urgent distance
    /// asking for them (out-of-view ones `OUT_OF_VIEW_M` further).
    want: HashMap<usize, f32>,
    /// Levels (slots) instances in view may be drawing now, and the coarsest of any within the
    /// draw distance: their models, or the resident ones standing in for them, must stay.
    keep: HashSet<usize>,
}

/// The plan for one asset with `lods` from its noted groups.
fn plan_levels<'a>(
    lods: &[Lod],
    groups: impl Iterator<Item = &'a Noted>,
    view: &ScanView,
) -> LevelPlan {
    let n = lods.len();
    let mut plan = LevelPlan::default();
    let slack_h = visioncraft_render::lod::HYSTERESIS_M * view.lod_scale;
    for g in groups {
        let ends = level_ends(lods, g.scale, view, g.capped);
        for &p in &g.at {
            let d = p.distance(view.eye);
            let need = level_at(&ends, d);
            if plan.nearest.is_none_or(|(nd, _)| d < nd) {
                plan.nearest = Some((d, need));
            }
            // The finest level the renderer may draw it with (its cluster nearer, hysteresis)
            // and the coarsest (hysteresis keeping a level further out).
            let lo = level_at(&ends, (d - CLUSTER_SLACK_M - slack_h).max(0.0));
            if lo >= n {
                continue;
            }
            let hi = level_at(&ends, d + slack_h).min(n - 1);
            let visible = view
                .view_proj
                .is_none_or(|vp| in_view(vp, p, CLUSTER_SLACK_M));
            let urgency = if visible { d } else { d + OUT_OF_VIEW_M };
            for i in (lo..=hi).chain([n - 1]) {
                let e = plan.want.entry(i).or_insert(f32::INFINITY);
                *e = e.min(urgency);
                // The coarsest, the fallback for everything, stays for those out of view too.
                if visible || i == n - 1 {
                    plan.keep.insert(i);
                }
            }
        }
    }
    plan
}

fn free(r: &mut Renderer, m: Model) {
    // Freed before the textures: its bind group holds them.
    for id in &m.materials {
        r.remove_material(*id);
    }
    m.meshes.into_iter().for_each(|id| r.remove_mesh(id));
    m.textures.into_iter().for_each(|id| r.remove_texture(id));
}

impl Library {
    pub fn new() -> Self {
        let mut store = AssetStore::new();
        for (path, bytes) in crate::fixtures::files() {
            store.provide(&path, Arc::from(bytes));
        }
        Self {
            store,
            catalog: None,
            materials: Vec::new(),
            skies: Vec::new(),
            tiers: None,
            texture_tier: "1k",
            tier_asked: Default::default(),
            need: HashMap::new(),
            tier_refused: HashSet::new(),
            catalog_arrived: false,
            user_assets: Vec::new(),
            ground_offsets: HashMap::new(),
            indexes_asked: false,
            models: HashMap::new(),
            awaiting: HashSet::new(),
            uploading: Vec::new(),
            garbage: Vec::new(),
            wants: HashMap::new(),
            lod_seen: HashMap::new(),
            unscanned: false,
            lamps_on: false,
            lamps_applied: None,
            rev: 0,
            specs: vec![TexSpec::default()],
            textured: Vec::new(),
            mem: HashMap::new(),
            mem_tex: HashMap::new(),
            resolved_store_rev: u64::MAX,
            dirty: false,
            messages: Vec::new(),
            worst_pump_ms: 0.0,
            plant_impostors: false,
            impostors: HashMap::new(),
            no_impostor: HashSet::new(),
            baking: None,
        }
    }

    /// A catalogue asset with models to show.
    pub fn knows(&self, id: &str) -> bool {
        self.catalog
            .as_ref()
            .and_then(|c| c.asset(id))
            .is_some_and(|a| !a.lods.is_empty())
    }

    pub fn resident_models(&self) -> usize {
        self.models.len()
    }

    // ---- what the scene tells us ----

    /// Records where one (asset, variant) group of a generator stands (for the distance
    /// priorities and what stays resident). `capped`: the group stops at the model draw
    /// distance (`caps_draw`), as its renderer item does.
    pub fn note(
        &mut self,
        generator: &str,
        asset: &str,
        variant: u32,
        matrices: &[Mat4],
        capped: bool,
    ) {
        let at: Vec<Vec3> = matrices.iter().map(|m| m.w_axis.truncate()).collect();
        // As `scene::scale_lod_distances`: the group's largest scale.
        let scale = matrices
            .iter()
            .map(|m| m.x_axis.length().max(m.y_axis.length()))
            .fold(1.0_f32, f32::max);
        self.unscanned = true;
        let list = self.wants.entry(generator.to_string()).or_default();
        let noted = Noted {
            asset: asset.to_string(),
            variant,
            at,
            scale,
            capped,
        };
        match list
            .iter_mut()
            .find(|e| e.asset == asset && e.variant == variant)
        {
            Some(e) => *e = noted,
            None => list.push(noted),
        }
    }

    /// Whether a catalogue asset stops drawing at the model draw distance: a library model (a
    /// bench, a pot, a lamp), not a plant (they have their own levels and impostors) and not an
    /// object the person imported (part of the design).
    pub fn caps_draw(&self, asset: &str) -> bool {
        self.catalog
            .as_ref()
            .and_then(|c| c.asset(asset))
            .is_some_and(|a| a.plant.is_none())
            && !self.user_assets.iter().any(|a| a.id == asset)
    }

    /// Which levels of `asset` are resident (the 2k nearest level counting for the first).
    fn resident_levels(&self, asset: &str, lods: &[Lod]) -> Vec<bool> {
        let tier = self
            .model_tier(asset)
            .filter(|f| self.models.contains_key(f));
        lods.iter()
            .enumerate()
            .map(|(i, l)| self.models.contains_key(&l.file) || (i == 0 && tier.is_some()))
            .collect()
    }

    /// A generator is being rebuilt or is gone: it notes again if it still needs anything.
    pub fn forget_generator(&mut self, generator: &str) {
        self.wants.remove(generator);
    }

    /// The resident parts of a catalogue asset as (mesh, material, max distance) of an LOD item:
    /// each level uses its own model, else the next coarser one in memory, else a finer one.
    /// `None` while nothing of it is resident (the caller keeps the placeholder).
    pub fn parts(&self, asset: &str) -> Option<Vec<(MeshId, MaterialId, f32)>> {
        let a = self.catalog.as_ref()?.asset(asset)?;
        let loaded: Vec<Option<&Model>> = a
            .lods
            .iter()
            .enumerate()
            .map(|(i, l)| {
                // The 2k nearest level stands in for the base one once it is resident.
                (i == 0)
                    .then(|| self.model_tier(asset))
                    .flatten()
                    .and_then(|f| self.models.get(&f))
                    .or_else(|| self.models.get(&l.file))
            })
            .collect();
        let resident: Vec<bool> = loaded.iter().map(Option::is_some).collect();
        let mut out = Vec::new();
        for (i, lod) in a.lods.iter().enumerate() {
            let m = loaded[shown_model(i, &resident)?]?;
            out.extend(
                m.parts
                    .iter()
                    .map(|&(mesh, mat)| (mesh, mat, lod.max_distance_m)),
            );
        }
        Some(out)
    }

    /// Which level of detail each placed catalogue asset shows and why, for agents (`ui.load`):
    /// the nearest instance's distance, the level that distance needs, the levels resident,
    /// the level drawn there, and in words why it is not the one needed.
    pub fn lod_report(&self) -> serde_json::Value {
        let Some(cat) = self.catalog.as_ref() else {
            return serde_json::json!([]);
        };
        let mut rows: Vec<(&String, &(f32, usize))> = self.lod_seen.iter().collect();
        rows.sort_by(|a, b| a.1.0.total_cmp(&b.1.0));
        let rows: Vec<serde_json::Value> = rows
            .into_iter()
            .filter_map(|(asset, &(d, need))| {
                let a = cat.asset(asset)?;
                let tier = (!a.lods.is_empty())
                    .then(|| self.model_tier(asset))
                    .flatten()
                    .filter(|f| self.models.contains_key(f));
                let is_resident = self.resident_levels(asset, &a.lods);
                let resident: Vec<usize> = (0..is_resident.len()).filter(|&i| is_resident[i]).collect();
                let beyond = need >= a.lods.len();
                // As `parts`: the level itself, else the next coarser resident one, else a finer.
                let shown = (!beyond).then(|| shown_model(need, &is_resident)).flatten();
                let file = &a.lods.get(need.min(a.lods.len().saturating_sub(1)))?.file;
                let why = if beyond {
                    "past the model draw distance: not drawn and not asked for".to_string()
                } else if shown == Some(need) {
                    "the level needed".to_string()
                } else if self.awaiting.contains(file)
                    || self.uploading.iter().any(|u| &u.path == file)
                {
                    "the level needed is loading".to_string()
                } else if self.store.state(&Key::Glb(file.clone())) == State::Failed {
                    "the level needed failed to load".to_string()
                } else if self.store.over_cap && need > 0 {
                    format!(
                        "over the texture memory cap ({} of {} MB): only the coarsest level and the nearest are asked for",
                        self.store.texture_bytes >> 20,
                        self.store.cap_bytes >> 20
                    )
                } else if shown.is_none() {
                    "nothing resident yet (placeholder)".to_string()
                } else {
                    "not asked for yet (the next scan)".to_string()
                };
                Some(serde_json::json!({
                    "asset": asset,
                    "nearest_m": (d * 10.0).round() / 10.0,
                    "levels": a.lods.len(),
                    "needed": need,
                    "resident": resident,
                    "shown": shown,
                    "tier_2k": tier.is_some(),
                    "why": why,
                }))
            })
            .collect();
        serde_json::Value::Array(rows)
    }

    // ---- textured project materials ----

    pub fn intern(&mut self, spec: TexSpec) -> usize {
        match self.specs.iter().position(|s| *s == spec) {
            Some(i) => i,
            None => {
                self.specs.push(spec);
                self.specs.len() - 1
            }
        }
    }

    /// A renderer material made for texture spec `spec` (an index from `intern`).
    pub fn register_textured(&mut self, material: MaterialId, base: Material, spec: usize) {
        self.textured.push(Textured {
            material,
            base,
            spec,
        });
        self.dirty = true;
    }

    /// A project material was freed: its maps are no longer applied to it.
    pub fn forget_textured(&mut self, material: MaterialId) {
        self.textured.retain(|t| t.material != material);
    }

    /// Decoded pixels (a dropped `.mcmat` map) usable as `TexSrc::Mem(name)`.
    pub fn add_memory_image(&mut self, name: String, img: Rgba) {
        self.mem.insert(name, img);
        self.dirty = true;
    }

    /// Marks the pictures of the project's materials as used (and asks again for any that were
    /// evicted), so memory pressure takes the models' finer levels before them.
    fn touch_textured(&mut self) {
        for i in 0..self.textured.len() {
            let mut spec = self.specs[self.textured[i].spec].clone();
            // Height maps only where the parallax uses them (not on walls and roofs).
            if self.textured[i].base.parallax_depth_m <= 0.0 {
                spec.height = None;
            }
            for (src, srgb) in [
                (&spec.base, true),
                (&spec.normal, false),
                (&spec.orm, false),
                (&spec.roughness, false),
                (&spec.metallic, false),
                (&spec.ao, false),
                (&spec.height, false),
            ] {
                if let Some(TexSrc::Lib(p)) = src {
                    let _ = self.store.texture(p, srgb, true, 10.0);
                    if let Some(t) = self.tier_map(p, self.textured[i].spec) {
                        let _ = self.store.texture(&t, srgb, true, 20.0);
                    }
                }
            }
        }
    }

    fn resolve_textured(&mut self, r: &mut Renderer) {
        for i in 0..self.textured.len() {
            let spec_index = self.textured[i].spec;
            let spec = self.specs[self.textured[i].spec].clone();
            let mut m = self.textured[i].base;
            let mut pending = false;
            let mut get =
                |this: &mut Self, src: &Option<TexSrc>, srgb: bool| -> Option<TextureId> {
                    match src {
                        None => None,
                        Some(TexSrc::Lib(p)) => {
                            // The base file at once, the higher tier in its place once it is
                            // resident (asked for only while it fits the memory budget).
                            let base = this.store.texture(p, srgb, true, 10.0);
                            let tier = this
                                .tier_map(p, spec_index)
                                .and_then(|t| this.store.texture(&t, srgb, true, 20.0));
                            pending |= base.is_none();
                            tier.or(base)
                        }
                        Some(TexSrc::Mem(n)) => {
                            if !this.mem_tex.contains_key(n)
                                && let Some(img) = this.mem.get(n)
                                && let Ok(id) = r.add_texture(&TextureDesc {
                                    width: img.w,
                                    height: img.h,
                                    rgba8: &img.px,
                                    color_space: if srgb {
                                        ColorSpace::Srgb
                                    } else {
                                        ColorSpace::Linear
                                    },
                                    repeat: true,
                                })
                            {
                                this.mem_tex.insert(n.clone(), id);
                            }
                            this.mem_tex.get(n).copied()
                        }
                    }
                };
            m.base_color_map = get(self, &spec.base, true);
            m.normal_map = get(self, &spec.normal, false);
            m.orm_map = get(self, &spec.orm, false);
            m.roughness_map = get(self, &spec.roughness, false);
            m.metallic_map = get(self, &spec.metallic, false);
            m.occlusion_map = get(self, &spec.ao, false);
            if m.parallax_depth_m > 0.0 {
                m.height_map = get(self, &spec.height, false);
            }
            if spec.base.is_some() && m.base_color_map.is_none() {
                // Until the picture arrives: a mid tone instead of a white flash.
                for c in &mut m.base_color[..3] {
                    *c *= 0.45;
                }
            }
            let _ = pending;
            r.set_material(self.textured[i].material, &m);
        }
    }

    /// The 2k nearest level of a catalogue model at the 2k and 4k tiers, unless it failed.
    fn model_tier(&self, asset: &str) -> Option<String> {
        if self.texture_tier == "1k" {
            return None;
        }
        let f = &self
            .tiers
            .as_ref()?
            .models
            .get(asset)?
            .tiers
            .get("2k")?
            .file;
        (self.store.state(&Key::Glb(f.clone())) != State::Failed && !self.tier_refused.contains(f))
            .then(|| f.clone())
    }

    fn model_tier_bytes(&self, asset: &str) -> u64 {
        self.tiers
            .as_ref()
            .and_then(|t| t.models.get(asset)?.tiers.get("2k"))
            .map_or(0, |t| t.bytes)
    }

    /// The tier a material's maps use: the texture tier, except that at 4k only materials on a
    /// surface near enough for 4096 texels per repeat to land on more than 2048 pixels get it
    /// (`need`, from `Scene::pump`); the others, and those out of view, get 2k.
    fn tier_for(&self, spec: usize) -> &'static str {
        match self.texture_tier {
            "4k" if self.need.get(&spec).is_some_and(|&n| n > 2048.0) => "4k",
            "4k" => "2k",
            t => t,
        }
    }

    /// Screen pixels per texture repeat each textured material spans at its nearest visible
    /// surface (`Scene::pump`); materials whose tier changes pick their maps again.
    pub fn set_need(&mut self, need: HashMap<usize, f32>) {
        let before: Vec<&'static str> = (0..self.specs.len()).map(|s| self.tier_for(s)).collect();
        self.need = need;
        if (0..self.specs.len()).any(|s| self.tier_for(s) != before[s]) {
            self.dirty = true;
        }
    }

    /// Sets the texture tier; materials pick their maps again.
    pub fn set_texture_tier(&mut self, tier: &'static str) {
        if self.texture_tier != tier {
            self.texture_tier = tier;
            self.tier_refused.clear();
            self.dirty = true;
        }
    }

    /// The higher-tier file for base map `p` at the current texture tier, when there is one and
    /// it is resident or fits the texture budget.
    fn tier_map(&self, p: &str, spec: usize) -> Option<String> {
        let tier = self.tier_for(spec);
        if tier == "1k" {
            return None;
        }
        if tier != "4k"
            && let Some((four, _)) = self.tiers.as_ref()?.material_map(p, "4k")
        {
            // No longer wanted at 4k (moved away, or out of view): a fetch not begun is dropped.
            self.store.cancel(four);
        }
        let (path, px) = self.tiers.as_ref()?.material_map(p, tier)?;
        let state = |srgb| {
            self.store.state(&Key::Image {
                path: path.to_string(),
                srgb,
                repeat: true,
            })
        };
        let (a, b) = (state(true), state(false));
        let ok = if a == State::Ready || b == State::Ready {
            self.tier_asked.borrow_mut().remove(path);
            true
        } else if a == State::Failed || b == State::Failed {
            self.tier_asked.borrow_mut().remove(path);
            false
        } else {
            if a == State::Absent && b == State::Absent {
                // Dropped at upload for want of room (or evicted): ask the budget again.
                self.tier_asked.borrow_mut().remove(path);
            }
            // Compressed maps take a quarter (BC5) to an eighth (BC1) of RGBA8.
            let cost =
                crate::assets::texture_cost(px, px) / if self.store.compress { 4 } else { 1 };
            self.budget_for(path, cost)
        };
        ok.then(|| path.to_string())
    }

    /// Whether a tier file of `bytes` may be asked for: already asked, or it fits the budget
    /// with everything asked and not yet resident counted (they all land in the same frames).
    fn budget_for(&self, path: &str, bytes: u64) -> bool {
        let mut asked = self.tier_asked.borrow_mut();
        if asked.contains_key(path) {
            return true;
        }
        let pending: u64 = asked.values().sum();
        let ok = self.store.fits(pending + bytes);
        if ok {
            asked.insert(path.to_string(), bytes);
        }
        ok
    }

    // ---- the frame ----

    fn read_indexes(&mut self) {
        let catalog = Key::Bytes("catalog.json".into());
        if let Some(Loaded::Bytes(b)) = self.store.take(&catalog) {
            match Catalog::from_json_bytes(&b) {
                Ok(c) => {
                    self.catalog = Some(Arc::new(c));
                    self.merge_user_assets();
                    self.catalog_arrived = true;
                    self.rev += 1;
                }
                Err(e) => self
                    .messages
                    .push(format!("The asset catalogue is unusable: {e}")),
            }
        }
        if let Some(Loaded::Bytes(b)) = self.store.take(&Key::Bytes("materials.json".into())) {
            match serde_json::from_slice::<MaterialsFile>(&b) {
                Ok(f) => self.materials = f.materials,
                Err(e) => self
                    .messages
                    .push(format!("materials.json is unusable: {e}")),
            }
        }
        if let Some(Loaded::Bytes(b)) = self.store.take(&Key::Bytes("tiers.json".into())) {
            match visioncraft_core::Tiers::from_json_bytes(&b) {
                Ok(t) => {
                    self.tiers = Some(Arc::new(t));
                    self.dirty = true;
                }
                Err(e) => self.messages.push(e),
            }
        }
        if let Some(Loaded::Bytes(b)) = self.store.take(&Key::Bytes("skies.json".into())) {
            match serde_json::from_slice::<SkiesFile>(&b) {
                Ok(f) => self.skies = f.skies,
                Err(e) => self.messages.push(format!("skies.json is unusable: {e}")),
            }
        }
    }

    /// Asks for a model that is not resident (what stays resident is up to `scan`'s keep:
    /// asking does not mark it used, so a level wanted only out of view may still go first).
    fn request(&mut self, path: &str, prio: f32) {
        let key = Key::Glb(path.to_string());
        if !self.models.contains_key(path) && !self.uploading.iter().any(|u| u.path == path) {
            self.awaiting.insert(path.to_string());
            self.store.want(key, prio);
        }
    }

    /// Asks for the levels the placed assets' instances show, nearest and in view first (the
    /// coarsest always, shown while a finer one loads), and keeps resident what instances in
    /// view draw now: a level's own model, or the one standing in for it until it arrives.
    /// Instances past the model draw distance ask for and keep nothing.
    fn scan(&mut self, view: &ScanView) {
        self.unscanned = false;
        let Some(cat) = self.catalog.clone() else {
            return;
        };
        self.lod_seen.clear();
        let mut groups: HashMap<&str, Vec<&Noted>> = HashMap::new();
        for g in self.wants.values().flatten() {
            groups.entry(g.asset.as_str()).or_default().push(g);
        }
        let plans: Vec<(String, LevelPlan)> = groups
            .into_iter()
            .filter_map(|(asset, list)| {
                let a = cat.asset(asset).filter(|a| !a.lods.is_empty())?;
                Some((
                    asset.to_string(),
                    plan_levels(&a.lods, list.into_iter(), view),
                ))
            })
            .collect();
        let mut pinned = HashSet::new();
        for (asset, plan) in plans {
            let Some(a) = cat.asset(&asset) else { continue };
            let n = a.lods.len();
            if let Some(near) = plan.nearest {
                self.lod_seen.insert(asset.clone(), near);
            }
            let mut want: Vec<(usize, f32)> = plan.want.into_iter().collect();
            want.sort_by_key(|w| w.0);
            for (i, d) in want {
                // Over the memory cap only the coarsest levels are asked for, and the nearest
                // one where the camera is right at the asset (else a bench at arm's length is
                // its coarsest slab). What is resident and shown stays all the same (`keep`):
                // dropping it made the level go and come back, round and round.
                if !(i == n - 1 || i == 0 || !self.store.over_cap) {
                    continue;
                }
                self.request(&a.lods[i].file, priority(d, i, n - 1));
                // Decoded, a model takes several times its file (its JPEG pictures unpacked, with mips).
                if i == 0
                    && let Some(f) = self.model_tier(&asset)
                    && self.budget_for(&f, self.model_tier_bytes(&asset) * 8)
                {
                    self.request(&f, priority(d, i, n - 1) + 1.0);
                }
            }
            let resident = self.resident_levels(&asset, &a.lods);
            for slot in plan.keep {
                let Some(m) = shown_model(slot, &resident) else {
                    continue;
                };
                let base_resident = self.models.contains_key(&a.lods[m].file);
                if m == 0
                    && let Some(f) = self.model_tier(&asset)
                    && self.models.contains_key(&f)
                {
                    let tier = Key::Glb(f);
                    self.store.touch(&tier);
                    // Shown with no base level under it: it may not go, not even over the cap.
                    if !base_resident {
                        pinned.insert(tier);
                    }
                }
                if base_resident {
                    self.store.touch(&Key::Glb(a.lods[m].file.clone()));
                }
            }
        }
        self.store.pin(pinned);
    }

    fn finish(&mut self, r: &mut Renderer, up: Upload) {
        let Upload {
            path,
            data,
            slots,
            ids,
            lumens,
        } = up;
        let bytes: u64 = slots
            .iter()
            .map(|&(i, _)| texture_cost(data.images[i].w, data.images[i].h))
            .sum();
        let tex = |img: Option<usize>, srgb: bool| {
            let i = img?;
            let s = slots.iter().position(|s| *s == (i, srgb))?;
            ids.get(s).copied()
        };
        let mut model = Model {
            parts: Vec::new(),
            emitters: Vec::new(),
            lumens,
            textures: ids.clone(),
            meshes: Vec::new(),
            materials: Vec::new(),
        };
        let mut made: HashMap<usize, MaterialId> = HashMap::new();
        // An imported object stands on its lowest point, wherever its file put the origin; the
        // offset is kept, the file is not changed.
        let lift = if self
            .user_assets
            .iter()
            .any(|a| a.lods.iter().any(|l| l.file == path))
        {
            let low = data
                .glb
                .prims
                .iter()
                .flat_map(|p| &p.positions)
                .map(|q| q[1])
                .filter(|y| y.is_finite())
                .fold(f32::INFINITY, f32::min);
            if low.is_finite() { -low } else { 0.0 }
        } else {
            0.0
        };
        self.ground_offsets.insert(path.clone(), lift);
        for p in &data.glb.prims {
            let lifted: Vec<[f32; 3]>;
            let positions = if lift == 0.0 {
                &p.positions
            } else {
                lifted = p
                    .positions
                    .iter()
                    .map(|q| [q[0], q[1] + lift, q[2]])
                    .collect();
                &lifted
            };
            let mat = match made.get(&p.material) {
                Some(m) => *m,
                None => {
                    let gm = &data.glb.materials[p.material];
                    let mut m = model_material(gm, &tex);
                    if gm.emitter {
                        m.emissive_strength = 0.0;
                    }
                    let id = r.add_material(&m);
                    if gm.emitter {
                        model.emitters.push((id, m));
                    }
                    model.materials.push(id);
                    made.insert(p.material, id);
                    id
                }
            };
            let mesh = r.add_mesh(&Mesh {
                positions,
                normals: &p.normals,
                indices: &p.indices,
                face_ids: None,
                uvs: Some(&p.uvs),
                wind: None,
            });
            match mesh {
                Ok(mesh) => {
                    model.meshes.push(mesh);
                    model.parts.push((mesh, mat));
                }
                Err(e) => self
                    .messages
                    .push(format!("A part of {path} is not drawn: {e}")),
            }
        }
        if model.parts.is_empty() {
            free(r, model);
            self.store
                .fail_key(Key::Glb(path), "it has nothing to draw");
            return;
        }
        if !model.emitters.is_empty() {
            self.lamps_applied = None;
        }
        self.store.track(Key::Glb(path.clone()), bytes);
        self.models.insert(path, model);
        self.rev += 1;
    }

    /// One frame of streaming: loads, uploads within the budget, evictions, requests from the
    /// camera position, lamps. Returns true when scene items must be rebuilt.
    pub fn pump(&mut self, r: &mut Renderer, eye: Vec3) -> bool {
        let t0 = now_ms();
        self.store.compress = r.supports_bc();
        let budget = Budget::new(4.0);
        let rev_before = self.rev;
        for m in std::mem::take(&mut self.garbage) {
            free(r, m);
        }
        if !self.indexes_asked {
            self.indexes_asked = true;
            for f in ["catalog.json", "materials.json", "skies.json"] {
                self.store.want(Key::Bytes(f.into()), 0.0);
            }
        }
        // The tier index only once a higher tier is wanted: the first load stays as light.
        if self.texture_tier != "1k" && self.tiers.is_none() {
            self.store.want(Key::Bytes("tiers.json".into()), 0.0);
        }
        self.store.pump(r, &budget);
        for key in self.store.take_evicted() {
            if let Key::Glb(p) = key
                && let Some(m) = self.models.remove(&p)
            {
                self.garbage.push(m);
                self.rev += 1;
            }
        }
        self.read_indexes();
        for path in self.awaiting.clone() {
            let key = Key::Glb(path.clone());
            match self.store.take(&key) {
                Some(Loaded::Glb(data)) => {
                    self.awaiting.remove(&path);
                    self.tier_asked.get_mut().remove(&path);
                    let cost: u64 = data
                        .images
                        .iter()
                        .map(|i| crate::assets::texture_cost(i.w, i.h))
                        .sum();
                    if crate::assets::is_tier_file(&path) && !self.store.fits(cost) {
                        // No room for the 2k model now: the base level stays, not asked again
                        // at this tier.
                        self.tier_refused.insert(path);
                        continue;
                    }
                    let slots = glb_slots(&data.glb);
                    let lumens = self.lumens_of(&path);
                    self.uploading.push(Upload {
                        path,
                        data,
                        slots,
                        ids: Vec::new(),
                        lumens,
                    });
                }
                _ if self.store.state(&key) == State::Failed => {
                    self.awaiting.remove(&path);
                    self.tier_asked.get_mut().remove(&path);
                }
                _ => {}
            }
        }
        while !self.uploading.is_empty() {
            let up = &mut self.uploading[0];
            if up.ids.len() < up.slots.len() {
                let (img, srgb) = up.slots[up.ids.len()];
                match self.store.upload(r, &up.data.images[img], srgb, true) {
                    Some(id) => up.ids.push(id),
                    None => {
                        let up = self.uploading.remove(0);
                        up.ids.into_iter().for_each(|id| r.remove_texture(id));
                        self.store
                            .fail_key(Key::Glb(up.path), "the GPU refused a texture");
                    }
                }
            } else {
                let up = self.uploading.remove(0);
                self.finish(r, up);
            }
            if budget.spent() {
                break;
            }
        }
        let tick = self.store.tick();
        if tick % 10 == 1 {
            self.scan(&ScanView::from_renderer(r, eye));
            self.touch_textured();
        }
        if self.lamps_applied != Some(self.lamps_on) {
            self.lamps_applied = Some(self.lamps_on);
            for m in self.models.values() {
                for (id, template) in &m.emitters {
                    let mut t = *template;
                    t.emissive_strength = if self.lamps_on {
                        emitter_luminance(m.lumens)
                    } else {
                        0.0
                    };
                    r.set_material(*id, &t);
                }
            }
        }
        if self.dirty || self.store.rev != self.resolved_store_rev {
            self.dirty = false;
            self.resolved_store_rev = self.store.rev;
            self.resolve_textured(r);
        }
        self.messages.append(&mut self.store.messages);
        let ms = now_ms() - t0;
        self.worst_pump_ms = self.worst_pump_ms.max(ms);
        if cfg!(target_arch = "wasm32") && tick.is_multiple_of(30) {
            self.publish_stats(r);
        }
        self.bake_plants(r, tick);
        self.rev != rev_before
    }

    /// Turns the catalogue plants' impostors on or off (off frees them). True when it changed:
    /// their items are built again.
    pub fn set_plant_impostors(&mut self, r: &mut Renderer, on: bool) -> bool {
        if on == self.plant_impostors {
            return false;
        }
        self.plant_impostors = on;
        self.baking = None;
        for (_, imp) in self.impostors.drain() {
            r.remove_impostor(imp);
        }
        self.no_impostor.clear();
        self.rev += 1;
        true
    }

    /// A catalogue plant's impostor once baked, and the plant's height (m).
    pub fn plant_impostor(&self, asset: &str) -> Option<(ImpostorId, f32)> {
        let imp = *self.impostors.get(asset)?;
        let a = self.catalog.as_ref()?.asset(asset)?;
        Some((imp, a.size_m[1]))
    }

    /// The parts an impostor of `asset` is baked from: a catalogue plant's model before its
    /// coarsest, once resident.
    fn impostor_source(&self, asset: &str) -> Option<Vec<(MeshId, MaterialId)>> {
        let a = self.catalog.as_ref()?.asset(asset)?;
        if a.plant.is_none() || a.lods.len() < 2 {
            return None;
        }
        let m = self.models.get(&a.lods[a.lods.len() - 2].file)?;
        Some(m.parts.clone())
    }

    /// A wanted catalogue plant whose impostor is still to be baked, with its source parts.
    fn next_bake(&self) -> Option<(String, Vec<(MeshId, MaterialId)>)> {
        self.wants
            .values()
            .flatten()
            .map(|g| &g.asset)
            .find_map(|a| {
                (!self.impostors.contains_key(a) && !self.no_impostor.contains(a))
                    .then(|| self.impostor_source(a).map(|src| (a.clone(), src)))
                    .flatten()
            })
    }

    /// One step of the catalogue plants' impostors: frees those no generator wants any more
    /// (now and then), else starts the next wanted plant's bake or draws a few of its views.
    fn bake_plants(&mut self, r: &mut Renderer, tick: u64) {
        if !self.plant_impostors {
            return;
        }
        if tick.is_multiple_of(60) {
            let wanted: HashSet<&String> =
                self.wants.values().flatten().map(|g| &g.asset).collect();
            let unwanted: Vec<String> = (self.impostors.keys())
                .filter(|a| !wanted.contains(a))
                .cloned()
                .collect();
            for a in unwanted {
                if let Some(imp) = self.impostors.remove(&a) {
                    r.remove_impostor(imp);
                }
            }
        }
        if self.baking.is_none() {
            let Some((asset, src)) = self.next_bake() else {
                return;
            };
            match r.start_impostor_bake(&src, None, crate::plants::IMPOSTOR_VIEWS) {
                Ok(bake) => self.baking = Some((asset, bake)),
                Err(_) => {
                    self.no_impostor.insert(asset);
                    return;
                }
            }
        }
        let Some((_, bake)) = self.baking.as_mut() else {
            return;
        };
        let Some(result) = r.impostor_bake_step(bake, crate::plants::BAKE_VIEWS_PER_FRAME) else {
            return;
        };
        let Some((asset, _)) = self.baking.take() else {
            return;
        };
        match result {
            Ok(imp) => {
                self.impostors.insert(asset, imp);
                self.rev += 1;
            }
            // A model went away during the bake, or the bake failed: it stays a mesh.
            Err(_) => {
                self.no_impostor.insert(asset);
            }
        }
    }

    fn lumens_of(&self, path: &str) -> f32 {
        self.catalog
            .as_ref()
            .and_then(|c| {
                c.assets
                    .iter()
                    .find(|a| a.lods.iter().any(|l| l.file == path))
            })
            .and_then(|a| a.light.as_ref())
            .map_or(400.0, |l| l.lumens)
    }

    /// The numbers the browser measurements read (`window.__vcStats`).
    #[cfg(target_arch = "wasm32")]
    fn publish_stats(&self, r: &Renderer) {
        let s = &self.store.stats;
        // What the texture memory is, by kind (the browser runs read it to find a peak).
        let breakdown: Vec<String> = r
            .stats()
            .texture_breakdown
            .iter()
            .map(|(k, b)| format!("\"{k}\":{b}"))
            .collect();
        let json = format!(
            "{{\"breakdown\":{{{}}},\"texture_bytes\":{},\"peak_texture_bytes\":{},\"uploads\":{},\"worst_upload_ms\":{:.2},\"total_upload_ms\":{:.1},\"evictions\":{},\"queued\":{},\"models\":{},\"worst_pump_ms\":{:.2}}}",
            breakdown.join(","),
            r.stats().texture_bytes,
            s.peak_texture_bytes,
            s.uploads,
            s.worst_upload_ms,
            s.total_upload_ms,
            s.evictions,
            self.store.queued() + self.uploading.len(),
            self.models.len(),
            self.worst_pump_ms
        );
        let _ = js_sys::Reflect::set(&js_sys::global(), &"__vcStats".into(), &json.into());
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn publish_stats(&self, _r: &Renderer) {}

    /// Everything resident or wanted has arrived (for scripts that wait for streaming).
    pub fn idle(&self) -> bool {
        !self.store.busy()
            && self.uploading.is_empty()
            && self.awaiting.is_empty()
            && !self.unscanned
            && (!self.plant_impostors || (self.baking.is_none() && self.next_bake().is_none()))
    }
}

/// The texture spec of a project material's `maps`: library paths, or `mcmat:` names for maps
/// the app holds in memory. The same path in `roughness`, `metallic` and `ambient_occlusion`
/// means one packed occlusion-roughness-metallic map (the core has no field for it yet).
pub fn spec_of(maps: &visioncraft_core::material::MaterialMaps) -> TexSpec {
    let src = |o: &Option<String>| {
        o.as_ref().map(|s| {
            if s.starts_with("mcmat:") {
                TexSrc::Mem(s.clone())
            } else {
                TexSrc::Lib(s.clone())
            }
        })
    };
    // Projects saved before `maps.orm` existed put the packed map in all three fields.
    let legacy = matches!((&maps.roughness, &maps.metallic, &maps.ambient_occlusion),
        (Some(a), Some(b), Some(c)) if a == b && b == c);
    let packed = maps.orm.is_some() || legacy;
    TexSpec {
        base: src(&maps.base_color),
        normal: src(&maps.normal),
        orm: if maps.orm.is_some() {
            src(&maps.orm)
        } else if legacy {
            src(&maps.roughness)
        } else {
            None
        },
        roughness: if packed { None } else { src(&maps.roughness) },
        metallic: if packed { None } else { src(&maps.metallic) },
        ao: if packed {
            None
        } else {
            src(&maps.ambient_occlusion)
        },
        height: src(&maps.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emitter_luminance_follows_lumens_within_limits() {
        assert!(emitter_luminance(800.0) > emitter_luminance(100.0));
        assert_eq!(emitter_luminance(0.0), 500.0);
        assert_eq!(emitter_luminance(1e9), 30_000.0);
    }

    #[test]
    fn a_plant_entry_without_evergreen_or_exposure_reads() {
        let entry = r#"{"assets":[{"id":"b","name":"b","category":"other","tags":[],"license":"CC0-1.0","origin":"BuiltIn","author":"a","source_url":"u","size_m":[1,1,1],"lods":[{"file":"m/b.glb","max_distance_m":10.0}],"thumbnail":"t.png","plant":{"mature_height_m":1.0,"mature_width_m":1.0,"evergreen":null,"bloom_months":[],"exposure":null},"light":null}]}"#;
        assert_eq!(
            Catalog::from_json_bytes(entry.as_bytes())
                .unwrap()
                .assets
                .len(),
            1
        );
    }

    /// Below HIGH a catalogue plant wanted by a generator gets an impostor, baked from its model
    /// before the coarsest; off, it is freed. A plant with one level, or no plant, gets none.
    #[test]
    fn catalogue_plants_get_their_impostors_below_high() {
        let Ok((device, queue)) = pollster::block_on(visioncraft_render::headless_device()) else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        let mut l = Library::new();
        let bollard = crate::fixtures::glb_path("vc-bollard");
        let bytes = crate::fixtures::files()
            .into_iter()
            .find(|(p, _)| *p == bollard)
            .expect("the bollard's model")
            .1;
        for f in ["t/near.glb", "t/far.glb"] {
            l.store.provide(f, Arc::from(bytes.clone()));
        }
        let entry = |id: &str, lods: &str, plant: bool| {
            format!(
                r#"{{"id":"{id}","name":"{id}","category":"shrub","tags":[],"license":"CC0-1.0","origin":"BuiltIn","author":"a","source_url":"u","size_m":[1,1.5,1],"lods":[{lods}],"thumbnail":"t.png","plant":{},"light":null}}"#,
                if plant {
                    r#"{"mature_height_m":1.5,"mature_width_m":1.0,"evergreen":null,"bloom_months":[],"exposure":null}"#
                } else {
                    "null"
                }
            )
        };
        let two = r#"{"file":"t/near.glb","max_distance_m":5.0},{"file":"t/far.glb","max_distance_m":3.4e38}"#;
        let one = r#"{"file":"t/near.glb","max_distance_m":3.4e38}"#;
        let json = format!(
            r#"{{"assets":[{},{},{}]}}"#,
            entry("shrub", two, true),
            entry("tuft", one, true),
            entry("stone", two, false)
        );
        l.catalog = Some(Arc::new(Catalog::from_json_bytes(json.as_bytes()).unwrap()));
        for a in ["shrub", "tuft", "stone"] {
            l.note("g", a, 0, &[Mat4::IDENTITY], false);
        }
        assert!(l.set_plant_impostors(&mut r, true));
        assert!(!l.set_plant_impostors(&mut r, true), "no change");
        for _ in 0..500 {
            l.pump(&mut r, Vec3::ZERO);
            if l.plant_impostor("shrub").is_some() && l.idle() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (_, h) = l.plant_impostor("shrub").expect("the shrub's impostor");
        assert_eq!(h, 1.5);
        assert!(l.plant_impostor("tuft").is_none() && l.plant_impostor("stone").is_none());
        assert!(l.idle(), "nothing left to bake");
        assert!(l.set_plant_impostors(&mut r, false));
        assert!(l.plant_impostor("shrub").is_none(), "freed at HIGH");
    }

    // ---- which levels stay resident and which are asked for (`scan`) ----

    /// A library with one catalogue asset per (id, plant) of three levels `<id>/near.glb` (to
    /// 10 m), `<id>/mid.glb` (to 30 m) and `<id>/far.glb` (beyond).
    fn three_levels(ids: &[(&str, bool)]) -> Library {
        let entry = |id: &str, plant: bool| {
            format!(
                r#"{{"id":"{id}","name":"{id}","category":"other","tags":[],"license":"CC0-1.0","origin":"BuiltIn","author":"a","source_url":"u","size_m":[1,1,1],"lods":[{{"file":"{id}/near.glb","max_distance_m":10.0}},{{"file":"{id}/mid.glb","max_distance_m":30.0}},{{"file":"{id}/far.glb","max_distance_m":3.4e38}}],"thumbnail":"t.png","plant":{},"light":null}}"#,
                if plant {
                    r#"{"mature_height_m":1.5,"mature_width_m":1.0,"evergreen":null,"bloom_months":[],"exposure":null}"#
                } else {
                    "null"
                }
            )
        };
        let list: Vec<String> = ids.iter().map(|(id, p)| entry(id, *p)).collect();
        let json = format!(r#"{{"assets":[{}]}}"#, list.join(","));
        let mut l = Library::new();
        l.catalog = Some(Arc::new(Catalog::from_json_bytes(json.as_bytes()).unwrap()));
        l
    }

    /// Makes `file` a resident model, last used now.
    fn resident(l: &mut Library, file: &str) {
        let m = Model {
            parts: Vec::new(),
            emitters: Vec::new(),
            lumens: 0.0,
            textures: Vec::new(),
            meshes: Vec::new(),
            materials: Vec::new(),
        };
        l.models.insert(file.to_string(), m);
        l.store.track(Key::Glb(file.to_string()), 1 << 20);
    }

    fn at(x: f32, z: f32) -> Mat4 {
        Mat4::from_translation(Vec3::new(x, 0.0, z))
    }

    fn view(lod_scale: f32) -> ScanView {
        ScanView {
            eye: Vec3::ZERO,
            lod_scale,
            draw_m: f32::INFINITY,
            view_proj: None,
        }
    }

    /// Scans once, a long while (more than the store's protection) after everything was used.
    fn scan_later(l: &mut Library, v: &ScanView) -> u64 {
        l.store.advance(1000);
        l.scan(v);
        l.store.tick()
    }

    fn used_at(l: &Library, file: &str) -> Option<u64> {
        l.store.last_used(&Key::Glb(file.into()))
    }

    /// ULTRA doubles every level distance in the renderer: a bench 15 m off draws its nearest
    /// level, so that level is what must stay, not the one 15 m needs at face value.
    #[test]
    fn the_level_drawn_at_the_renderers_scale_stays_resident() {
        let mut l = three_levels(&[("bench", false)]);
        for f in ["bench/near.glb", "bench/mid.glb", "bench/far.glb"] {
            resident(&mut l, f);
        }
        l.note("g", "bench", 0, &[at(0.0, -25.0)], true);
        let now = scan_later(&mut l, &view(2.0));
        assert_eq!(
            used_at(&l, "bench/near.glb"),
            Some(now),
            "drawn at 25 m on ULTRA"
        );
        // A bench drawn at twice its size keeps each level twice as far, too.
        let mut l = three_levels(&[("bench", false)]);
        for f in ["bench/near.glb", "bench/mid.glb", "bench/far.glb"] {
            resident(&mut l, f);
        }
        let big = at(0.0, -25.0) * Mat4::from_scale(Vec3::splat(2.0));
        l.note("g", "bench", 0, &[big], true);
        let now = scan_later(&mut l, &view(1.0));
        assert_eq!(
            used_at(&l, "bench/near.glb"),
            Some(now),
            "drawn at 25 m at 2x"
        );
    }

    /// The variants of an asset in one planting are separate groups: the near ones count.
    #[test]
    fn every_variant_of_an_asset_counts() {
        let mut l = three_levels(&[("bench", false)]);
        for f in ["bench/near.glb", "bench/mid.glb", "bench/far.glb"] {
            resident(&mut l, f);
        }
        l.note("g", "bench", 0, &[at(0.0, -5.0)], true);
        l.note("g", "bench", 1, &[at(0.0, -60.0)], true);
        let now = scan_later(&mut l, &view(1.0));
        assert_eq!(used_at(&l, "bench/near.glb"), Some(now), "variant 0 at 5 m");
        // Noting a group again replaces only that group.
        l.note("g", "bench", 1, &[at(0.0, -70.0)], true);
        let now = scan_later(&mut l, &view(1.0));
        assert_eq!(used_at(&l, "bench/near.glb"), Some(now));
    }

    /// Walking up to a bench: the nearest level is asked for, and the level drawn in its place
    /// meanwhile stays until it arrives.
    #[test]
    fn the_stand_in_stays_until_the_finer_level_arrives() {
        let mut l = three_levels(&[("bench", false)]);
        resident(&mut l, "bench/mid.glb");
        resident(&mut l, "bench/far.glb");
        l.note("g", "bench", 0, &[at(0.0, -5.0)], true);
        let now = scan_later(&mut l, &view(1.0));
        assert_eq!(
            l.store.state(&Key::Glb("bench/near.glb".into())),
            State::Pending
        );
        assert_eq!(
            used_at(&l, "bench/mid.glb"),
            Some(now),
            "drawn until near arrives"
        );
    }

    /// Over the memory cap no finer level is asked for, but the one on screen is not dropped
    /// (it went, was asked for again under the cap, came back: round and round).
    #[test]
    fn over_the_cap_the_level_on_screen_stays() {
        let mut l = three_levels(&[("bench", false)]);
        for f in ["bench/mid.glb", "bench/far.glb"] {
            resident(&mut l, f);
        }
        l.store.over_cap = true;
        l.note("g", "bench", 0, &[at(0.0, -20.0)], true);
        let now = scan_later(&mut l, &view(1.0));
        assert_eq!(used_at(&l, "bench/mid.glb"), Some(now));
        assert_eq!(used_at(&l, "bench/far.glb"), Some(now));
    }

    /// What is in view outranks what is behind the camera: its levels stay and load first.
    #[test]
    fn instances_in_view_outrank_those_behind() {
        let eye = Vec3::new(0.0, 1.7, 0.0);
        let vp = Mat4::perspective_infinite_rh(1.0, 16.0 / 9.0, 0.1)
            * Mat4::look_at_rh(eye, Vec3::new(0.0, 1.7, -1.0), Vec3::Y);
        let v = ScanView {
            eye,
            lod_scale: 1.0,
            draw_m: f32::INFINITY,
            view_proj: Some(vp),
        };
        let mut l = three_levels(&[("front", false), ("back", false)]);
        for f in [
            "front/mid.glb",
            "front/far.glb",
            "back/mid.glb",
            "back/far.glb",
        ] {
            resident(&mut l, f);
        }
        l.note("g", "front", 0, &[at(0.0, -20.0)], true);
        l.note("g", "back", 0, &[at(0.0, 40.0)], true);
        let now = scan_later(&mut l, &v);
        assert_eq!(used_at(&l, "front/mid.glb"), Some(now), "in view");
        assert!(
            used_at(&l, "back/mid.glb") < Some(now),
            "behind: free to go first"
        );
        assert_eq!(used_at(&l, "back/far.glb"), Some(now), "its coarsest stays");
        // Nothing resident: the level in view is asked for before the same one behind.
        let mut l = three_levels(&[("front", false), ("back", false)]);
        l.note("g", "front", 0, &[at(0.0, -20.0)], true);
        l.note("g", "back", 0, &[at(0.0, 40.0)], true);
        l.scan(&v);
        let p = |f: &str| l.store.queued_priority(&Key::Glb(f.into())).unwrap();
        assert!(p("front/mid.glb") < p("back/mid.glb"));
    }

    #[test]
    fn in_view_tests_the_side_planes_with_a_margin() {
        let vp = Mat4::perspective_infinite_rh(1.0, 1.0, 0.1)
            * Mat4::look_at_rh(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
        assert!(in_view(vp, Vec3::new(0.0, 0.0, -50.0), 0.0));
        assert!(!in_view(vp, Vec3::new(0.0, 0.0, 50.0), 1.0), "behind");
        assert!(
            !in_view(vp, Vec3::new(100.0, 0.0, -10.0), 1.0),
            "far to the side"
        );
        assert!(
            in_view(vp, Vec3::new(0.0, 0.0, 5.0), 12.0),
            "just behind, within the margin"
        );
    }

    #[test]
    fn a_level_draws_its_own_model_else_a_coarser_else_a_finer() {
        assert_eq!(shown_model(0, &[true, true, true]), Some(0));
        assert_eq!(shown_model(0, &[false, true, true]), Some(1));
        assert_eq!(shown_model(1, &[true, false, false]), Some(0));
        assert_eq!(shown_model(1, &[false, false, false]), None);
    }

    /// Past the model draw distance a library model asks for nothing and keeps nothing; a
    /// plant, a scanned log (noted uncapped) and an imported object have no such limit.
    #[test]
    fn past_the_draw_distance_a_model_is_neither_asked_for_nor_kept() {
        let mut l = three_levels(&[("bench", false), ("shrub", true)]);
        assert!(l.caps_draw("bench") && !l.caps_draw("shrub"));
        resident(&mut l, "bench/far.glb");
        l.note("g", "bench", 0, &[at(0.0, -150.0)], l.caps_draw("bench"));
        l.note("g", "shrub", 0, &[at(0.0, -150.0)], l.caps_draw("shrub"));
        let v = ScanView {
            draw_m: 120.0,
            ..view(1.0)
        };
        let now = scan_later(&mut l, &v);
        for f in ["bench/near.glb", "bench/mid.glb"] {
            assert_eq!(l.store.state(&Key::Glb(f.into())), State::Absent, "{f}");
        }
        assert!(
            used_at(&l, "bench/far.glb") < Some(now),
            "not kept past 120 m"
        );
        assert_eq!(
            l.store.state(&Key::Glb("shrub/far.glb".into())),
            State::Pending,
            "plants are not capped"
        );
        assert_eq!(l.lod_seen["bench"].1, 3, "reported past the draw distance");
        // Within the distance it streams as before.
        l.note("g", "bench", 0, &[at(0.0, -100.0)], true);
        let now = scan_later(&mut l, &v);
        assert_eq!(used_at(&l, "bench/far.glb"), Some(now));
        // An imported object is part of the design.
        l.user_assets = l.catalog.as_ref().unwrap().assets.clone();
        assert!(!l.caps_draw("bench"));
    }

    /// The 2k nearest level on screen with no base level resident under it may not go.
    #[test]
    fn a_2k_model_shown_without_its_base_is_pinned() {
        let mut l = three_levels(&[("bench", false)]);
        l.tiers = Some(Arc::new(
            visioncraft_core::Tiers::from_json_bytes(
                br#"{"models":{"bench":{"tiers":{"2k":{"file":"bench/lod0_2k.glb","bytes":10}}}}}"#,
            )
            .unwrap(),
        ));
        l.texture_tier = "2k";
        for f in ["bench/lod0_2k.glb", "bench/mid.glb", "bench/far.glb"] {
            resident(&mut l, f);
        }
        l.note("g", "bench", 0, &[at(0.0, -5.0)], true);
        let tier = Key::Glb("bench/lod0_2k.glb".into());
        let now = scan_later(&mut l, &view(1.0));
        assert!(l.store.is_pinned(&tier));
        assert_eq!(l.store.last_used(&tier), Some(now));
        // Once the base level is resident the 2k one may go over the cap (it stands in).
        resident(&mut l, "bench/near.glb");
        scan_later(&mut l, &view(1.0));
        assert!(!l.store.is_pinned(&tier));
    }

    #[test]
    fn texture_specs_are_interned() {
        let mut l = Library::new();
        let a = TexSpec {
            base: Some(TexSrc::Lib("a.jpg".into())),
            ..Default::default()
        };
        let i = l.intern(a.clone());
        assert_eq!(l.intern(a), i);
        assert_eq!(l.intern(TexSpec::default()), 0);
        assert_ne!(i, 0);
    }

    #[test]
    fn indexes_parse_the_documented_shapes() {
        let m: MaterialsFile = serde_json::from_str(
            r#"{"materials":[{"id":"a","name":"A","tags":["x"],"kind":"brick","license":"CC0-1.0","author":"me","source_url":"u","size_m":[2.0,2.0],"maps":{"basecolor":"m/a.jpg","normal":"m/n.jpg","orm":"m/o.jpg","height":null},"thumbnail":"t.jpg"}]}"#,
        )
        .unwrap();
        assert_eq!(m.materials[0].maps.orm.as_deref(), Some("m/o.jpg"));
        assert!(m.materials[0].maps.height.is_none());
        let s: SkiesFile = serde_json::from_str(
            r#"{"skies":[{"id":"s","name":"S","license":"CC0-1.0","author":"a","source_url":"u","file":"f.hdr","thumbnail":"t.jpg"}]}"#,
        )
        .unwrap();
        assert_eq!(s.skies[0].file, "f.hdr");
    }
}
