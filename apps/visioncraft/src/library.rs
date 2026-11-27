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
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use visioncraft_core::catalog::Origin;
use visioncraft_core::keyframes::{Matrix, model_space};
use visioncraft_core::{Animation, Catalog};
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

pub(crate) struct Model {
    /// Tells one loaded model from another (`AssetRig`).
    pub(crate) id: u64,
    parts: Vec<(MeshId, MaterialId)>,
    /// Emitting materials with their template (strength 0), for the lamps toggle.
    emitters: Vec<(MaterialId, Material)>,
    lumens: f32,
    textures: Vec<TextureId>,
    meshes: Vec<MeshId>,
    materials: Vec<MaterialId>,
    /// The moving parts with their triangles, in the rig's order: a part without triangles (a
    /// skeleton joint) has none and gets no item. Empty for a model that stands still.
    pub(crate) bones: Vec<Bone>,
    /// Metres the whole model is raised by to stand on the ground (an imported object), in
    /// model space before its pose.
    pub(crate) lift: f32,
}

impl Model {
    /// The node of each moving part and its name, as [`Glb::nodes`] has it: what the first
    /// level's bones are matched to this level's by ([`crate::gltf_load::match_level`]).
    pub(crate) fn nodes(&self) -> Vec<(usize, String)> {
        self.bones
            .iter()
            .map(|b| (b.node, b.name.clone()))
            .collect()
    }
}

/// One moving part of a loaded model ([`Model::bones`]).
pub(crate) struct Bone {
    /// The part's node in the file, and its name (empty without one): what tells it from
    /// another level of detail's part, by name first ([`crate::gltf_load::part_key`]), by
    /// index where a name is missing or appears twice.
    pub(crate) node: usize,
    pub(crate) name: String,
    /// Its prims in the node's own space.
    pub(crate) prims: Vec<(MeshId, MaterialId)>,
    /// The part's matrix in model space at rest.
    pub(crate) rest_space: Matrix,
}

/// The animation of a model file, kept from its first load for as long as the session lasts
/// ([`Library::animations`]): what poses a catalogue asset and what `query.clips` reads,
/// whether or not the model is still resident.
pub(crate) struct Kept {
    /// Tells one kept animation from another: what a scene group's moving parts are built
    /// under.
    pub(crate) id: u64,
    /// The file's animation: its clips, what was left out of them, and the bones they move
    /// (`None` in `rig` for a model that stands still).
    pub(crate) anim: Animation,
    /// The glTF node of each bone ([`Bone::node`], and its name): how a level of detail's file
    /// knows the same part, by name first ([`crate::gltf_load::match_level`]).
    pub(crate) nodes: Vec<(usize, String)>,
    /// Metres the model is raised by, as [`Model::lift`] when it is resident.
    pub(crate) lift: f32,
}

/// An animated catalogue asset as the scene draws it: per level of detail the model drawn
/// there with the level's distance, and the first level's animation ([`AssetRig::authority`]),
/// which poses every level whether or not the first level is resident.
pub(crate) struct AssetRig<'a> {
    /// The first level's animation ([`Library::authority`]): `None` until its file was read
    /// once, then every part is drawn at rest.
    pub(crate) authority: Option<&'a Kept>,
    pub(crate) levels: Vec<(&'a Model, f32)>,
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
    let used: HashSet<usize> = glb
        .prims
        .iter()
        .chain(glb.parts.iter().flat_map(|part| &part.prims))
        .map(|p| p.material)
        .collect();
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
    /// The animation of every model file loaded so far, kept when the model is evicted: what
    /// `query.clips` and playback read in a browser, which has no file to read again, and the
    /// first level of detail's entry is what poses a catalogue asset ([`Self::authority`]).
    animations: HashMap<String, Kept>,
    /// The id of the next loaded model (`Model::id`).
    next_model_id: u64,
    /// The id of the next kept animation (`Kept::id`).
    next_anim_id: u64,
    awaiting: HashSet<String>,
    uploading: Vec<Upload>,
    garbage: Vec<Model>,
    /// Where each generator's library assets stand, to see how near the camera they are.
    wants: HashMap<String, Vec<(String, Vec<Vec3>)>>,
    /// Per placed catalogue asset at the last scan: its nearest instance's distance (m) and the
    /// level of detail it needs there (`lod_report`).
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
            animations: HashMap::new(),
            next_model_id: 0,
            next_anim_id: 0,
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

    /// Records where a generator's library assets stand (for the distance priorities).
    pub fn note(&mut self, generator: &str, asset: &str, matrices: &[Mat4]) {
        let at: Vec<Vec3> = matrices.iter().map(|m| m.w_axis.truncate()).collect();
        self.unscanned = true;
        let list = self.wants.entry(generator.to_string()).or_default();
        match list.iter_mut().find(|(a, _)| a == asset) {
            Some(e) => e.1 = at,
            None => list.push((asset.to_string(), at)),
        }
    }

    /// A generator is being rebuilt or is gone: it notes again if it still needs anything.
    pub fn forget_generator(&mut self, generator: &str) {
        self.wants.remove(generator);
    }

    /// Per level of detail of a catalogue asset the loaded model drawn there and the level's
    /// distance: each level uses its own model, else the next coarser one in memory, else a
    /// finer one. `None` while nothing of it is resident (the caller keeps the placeholder).
    fn levels(&self, asset: &str) -> Option<Vec<(&Model, f32)>> {
        let a = self.catalog.as_ref()?.asset(asset)?;
        let n = a.lods.len();
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
        let mut out = Vec::new();
        for (i, lod) in a.lods.iter().enumerate() {
            let m = (i..n)
                .find_map(|j| loaded[j])
                .or_else(|| (0..i).rev().find_map(|j| loaded[j]))?;
            out.push((m, lod.max_distance_m));
        }
        Some(out)
    }

    /// The resident parts of a catalogue asset as (mesh, material, max distance) of an LOD item:
    /// what stands still in it, from [`Self::levels`]. Empty for a model all of which moves
    /// (its parts are items of their own, [`Self::rig`]).
    pub fn parts(&self, asset: &str) -> Option<Vec<(MeshId, MaterialId, f32)>> {
        Some(
            self.levels(asset)?
                .iter()
                .flat_map(|(m, d)| m.parts.iter().map(move |&(mesh, mat)| (mesh, mat, *d)))
                .collect(),
        )
    }

    /// An animated catalogue asset as the scene draws it ([`AssetRig`]): per level of detail
    /// what is drawn there, and the first level's animation that poses them. `None` while
    /// nothing of it is resident (the caller keeps the placeholder).
    pub(crate) fn rig(&self, asset: &str) -> Option<AssetRig<'_>> {
        Some(AssetRig {
            authority: self.authority(asset),
            levels: self.levels(asset)?,
        })
    }

    /// The first level of detail's animation of a catalogue asset, from its file's first load.
    /// It is the authority over the asset: its clips are the asset's, and its bones and their
    /// rest poses pose every level, whatever level is resident. `None` until the file was read
    /// once: then nothing poses the asset, and its parts are drawn at rest.
    pub(crate) fn authority(&self, asset: &str) -> Option<&Kept> {
        let first = self.catalog.as_ref()?.asset(asset)?.lods.first()?;
        self.animations.get(&first.file)
    }

    /// Asks for the first level's file of a catalogue asset whose animation is not known yet (a
    /// clip is about to play on it, or `query.clips` asks): once read, that level's rig poses
    /// the asset whatever level is resident. Nothing for an imported object, whose file is the
    /// host's.
    pub fn want_animation(&mut self, asset: &str) {
        let first = self
            .catalog
            .as_ref()
            .and_then(|c| c.asset(asset))
            .filter(|a| a.origin != Origin::User)
            .and_then(|a| a.lods.first().map(|l| (l.file.clone(), a.lods.len())));
        let Some((file, levels)) = first else {
            return;
        };
        if !self.animations.contains_key(&file) {
            self.request(&file, priority(0.0, 0, levels - 1));
        }
    }

    /// The animation of a model file loaded once (its clips, what was left out, and its
    /// bones), whether or not it is still resident.
    pub fn animation(&self, file: &str) -> Option<&Animation> {
        self.animations.get(file).map(|k| &k.anim)
    }

    /// All that is kept of a model file loaded once ([`Kept`]), whether or not it is still
    /// resident.
    pub(crate) fn kept(&self, file: &str) -> Option<&Kept> {
        self.animations.get(file)
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
                let resident: Vec<usize> = a
                    .lods
                    .iter()
                    .enumerate()
                    .filter(|(i, l)| self.models.contains_key(&l.file) || (*i == 0 && tier.is_some()))
                    .map(|(i, _)| i)
                    .collect();
                // As `parts`: the level itself, else the next coarser resident one, else a finer.
                let shown = (need..a.lods.len())
                    .find(|i| resident.contains(i))
                    .or_else(|| (0..need).rev().find(|i| resident.contains(i)));
                let file = &a.lods.get(need)?.file;
                let why = if shown == Some(need) {
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

    fn request(&mut self, path: &str, prio: f32) {
        let key = Key::Glb(path.to_string());
        if self.models.contains_key(path) {
            self.store.touch(&key);
        } else if !self.uploading.iter().any(|u| u.path == path) {
            self.awaiting.insert(path.to_string());
            self.store.want(key, prio);
        }
    }

    /// Asks for the LOD each placed asset needs at its nearest instance, and for its coarsest
    /// (shown while the finer one loads), nearest first.
    fn scan(&mut self, eye: Vec3) {
        self.unscanned = false;
        let Some(cat) = self.catalog.clone() else {
            return;
        };
        self.lod_seen.clear();
        let mut nearest: HashMap<String, f32> = HashMap::new();
        for (asset, points) in self.wants.values().flatten() {
            let stride = (points.len() / 256).max(1);
            let d = points
                .iter()
                .step_by(stride)
                .map(|p| p.distance(eye))
                .fold(f32::INFINITY, f32::min);
            let e = nearest.entry(asset.clone()).or_insert(f32::INFINITY);
            *e = e.min(d);
        }
        for (asset, d) in nearest {
            let Some(a) = cat.asset(&asset) else { continue };
            let n = a.lods.len();
            let need = a
                .lods
                .iter()
                .position(|l| d <= l.max_distance_m)
                .unwrap_or(n - 1);
            self.lod_seen.insert(asset.clone(), (d, need));
            for (i, l) in a.lods.iter().enumerate() {
                // Over the memory cap only the coarsest levels are asked for, and the nearest
                // one where the camera is right at the asset (else a bench at arm's length is
                // its coarsest slab); the finer ones that are resident then stop being used and
                // are the first to go.
                if i == n - 1 || (i == need && (need == 0 || !self.store.over_cap)) {
                    self.request(&l.file, priority(d, i, n - 1));
                    // Decoded, a model takes several times its file (its JPEG pictures unpacked, with mips).
                    if i == 0
                        && let Some(f) = self.model_tier(&asset)
                        && self.budget_for(&f, self.model_tier_bytes(&asset) * 8)
                    {
                        self.request(&f, priority(d, i, n - 1) + 1.0);
                    }
                }
            }
        }
    }

    /// Uploads a model file: what stands still as one mesh per material, each moving part as
    /// its own meshes in the part's space ([`Model::bones`], posed by the scene).
    fn finish(&mut self, r: &mut Renderer, up: Upload) {
        let Upload {
            path,
            mut data,
            slots,
            ids,
            lumens,
        } = up;
        let glb = &mut data.glb;
        // An imported object stands on its lowest point, wherever its file put the origin; the
        // offset is kept, the file is not changed. Measured at its rest pose: bounds at rest.
        let lift = if self
            .user_assets
            .iter()
            .any(|a| a.lods.iter().any(|l| l.file == path))
        {
            // The same function the animation's `rest_low_y` comes from, so a headless host
            // answers the lift the window draws with.
            visioncraft_core::playback::lowest_y(
                glb.rest_prims()
                    .iter()
                    .flat_map(|p| &p.positions)
                    .map(|q| q[1]),
            )
            .map_or(0.0, |low| -low)
        } else {
            0.0
        };
        self.ground_offsets.insert(path.clone(), lift);
        let bytes: u64 = slots
            .iter()
            .map(|&(i, _)| texture_cost(data.images[i].w, data.images[i].h))
            .sum();
        let tex = |img: Option<usize>, srgb: bool| {
            let i = img?;
            let s = slots.iter().position(|s| *s == (i, srgb))?;
            ids.get(s).copied()
        };
        self.next_model_id += 1;
        let mut model = Model {
            id: self.next_model_id,
            parts: Vec::new(),
            emitters: Vec::new(),
            lumens,
            textures: ids.clone(),
            meshes: Vec::new(),
            materials: Vec::new(),
            bones: Vec::new(),
            lift,
        };
        let mut made: HashMap<usize, MaterialId> = HashMap::new();
        for p in glb.prims.iter() {
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
            let mat = self.material_of(r, &mut model, &mut made, glb, &tex, p.material);
            match r.add_mesh(&Mesh {
                positions,
                normals: &p.normals,
                indices: &p.indices,
                face_ids: None,
                uvs: Some(&p.uvs),
                wind: None,
            }) {
                Ok(mesh) => {
                    model.meshes.push(mesh);
                    model.parts.push((mesh, mat));
                }
                Err(e) => self
                    .messages
                    .push(format!("A part of {path} is not drawn: {e}")),
            }
        }
        if let Some(rig) = glb.rig() {
            let rest_space = model_space(&rig.parents, &rig.rest);
            for (part, rest_space) in glb.parts.iter().zip(&rest_space) {
                let mut prims = Vec::new();
                for p in &part.prims {
                    let mat = self.material_of(r, &mut model, &mut made, glb, &tex, p.material);
                    match r.add_mesh(&Mesh {
                        positions: &p.positions,
                        normals: &p.normals,
                        indices: &p.indices,
                        face_ids: None,
                        uvs: Some(&p.uvs),
                        wind: None,
                    }) {
                        Ok(mesh) => {
                            model.meshes.push(mesh);
                            prims.push((mesh, mat));
                        }
                        Err(e) => self
                            .messages
                            .push(format!("A part of {path} is not drawn: {e}")),
                    }
                }
                model.bones.push(Bone {
                    node: part.node,
                    name: part.name.clone(),
                    prims,
                    rest_space: *rest_space,
                });
            }
        }
        if model.meshes.is_empty() {
            free(r, model);
            self.store
                .fail_key(Key::Glb(path), "it has nothing to draw");
            return;
        }
        if !model.emitters.is_empty() {
            self.lamps_applied = None;
        }
        self.store.track(Key::Glb(path.clone()), bytes);
        self.keep_animation(&path, glb.animation(), glb.nodes(), lift);
        self.models.insert(path, model);
        self.rev += 1;
    }

    /// Keeps a model file's animation once it is loaded, for as long as the session lasts
    /// ([`Self::animations`]): the same file again keeps its id, so what was built under it
    /// stays, unless the file changed.
    fn keep_animation(
        &mut self,
        path: &str,
        anim: Animation,
        nodes: Vec<(usize, String)>,
        lift: f32,
    ) {
        match self.animations.entry(path.to_string()) {
            Entry::Occupied(mut e) => {
                let kept = e.get_mut();
                if kept.anim != anim {
                    self.next_anim_id += 1;
                    kept.id = self.next_anim_id;
                }
                kept.anim = anim;
                kept.nodes = nodes;
                kept.lift = lift;
            }
            Entry::Vacant(e) => {
                self.next_anim_id += 1;
                e.insert(Kept {
                    id: self.next_anim_id,
                    anim,
                    nodes,
                    lift,
                });
            }
        }
    }

    /// The renderer material of glTF material `index`, made once per model.
    fn material_of(
        &mut self,
        r: &mut Renderer,
        model: &mut Model,
        made: &mut HashMap<usize, MaterialId>,
        glb: &Glb,
        tex: &dyn Fn(Option<usize>, bool) -> Option<TextureId>,
        index: usize,
    ) -> MaterialId {
        if let Some(m) = made.get(&index) {
            return *m;
        }
        let gm = &glb.materials[index];
        let mut m = model_material(gm, tex);
        if gm.emitter {
            m.emissive_strength = 0.0;
        }
        let id = r.add_material(&m);
        if gm.emitter {
            model.emitters.push((id, m));
        }
        model.materials.push(id);
        made.insert(index, id);
        id
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
            self.scan(eye);
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
        self.wants.values().flatten().find_map(|(a, _)| {
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
            let wanted: HashSet<&String> = self.wants.values().flatten().map(|(a, _)| a).collect();
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

    /// A model's clips outlive the model: once it was loaded, `query.clips` and playback still
    /// know them after memory pressure evicted it (a browser has no file to read them again).
    #[test]
    fn a_model_s_clips_are_known_after_it_is_evicted() {
        let Ok((device, queue)) = pollster::block_on(visioncraft_render::headless_device()) else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        let mut l = Library::new();
        l.merge_user_assets();
        l.note("g", "vc-bollard", &[Mat4::IDENTITY]);
        for _ in 0..500 {
            l.pump(&mut r, Vec3::ZERO);
            if l.resident_models() == 1 && l.idle() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(l.resident_models(), 1, "the bollard is loaded");
        let file = crate::fixtures::glb_path("vc-bollard");
        let loaded = l.animation(&file).cloned().expect("its animation");
        assert!(loaded.clips.is_empty(), "the bollard stands still");
        l.store.evict(crate::assets::Key::Glb(file.clone()));
        l.pump(&mut r, Vec3::ZERO);
        assert_eq!(l.resident_models(), 0, "evicted");
        assert_eq!(
            l.animation(&file),
            Some(&loaded),
            "its clips are still known"
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
            l.note("g", a, &[Mat4::IDENTITY]);
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
