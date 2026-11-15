//! Real plants from `visioncraft-flora`: generated lazily (textures of one species, or the meshes
//! of one species and season class, per frame), uploaded once, drawn as LOD items.

use crate::scene::{self, MaterialId, MeshId};
use std::collections::HashMap;
use visioncraft_core::Project;
use visioncraft_flora as flora;
use visioncraft_render::{ImpostorId, Renderer, TextureId};

/// Forms per species, as the planting tools spread them over neighbouring plants.
pub const VARIANTS: u32 = visioncraft_core::planting::PLANT_VARIANTS;

pub struct Resolved {
    pub params: flora::PlantParams,
    pub category: flora::Category,
    /// The built-in species whose scanned leaves and bark it uses.
    pub scan_id: Option<String>,
}

/// The generator parameters of a palette asset id: `flora:<id>` or a custom species of the project.
pub fn resolve(project: &Project, asset: &str) -> Option<Resolved> {
    if let Some(id) = asset.strip_prefix("flora:") {
        let s = flora::Species::by_id(id)?;
        return Some(Resolved {
            params: s.params.clone(),
            category: s.category,
            scan_id: Some(id.to_string()),
        });
    }
    let c = project.species.iter().find(|c| c.id == asset)?;
    let params: flora::PlantParams = serde_json::from_value(c.params.clone()).ok()?;
    let category = c
        .base
        .as_deref()
        .and_then(flora::Species::by_id)
        .map_or(flora::Category::Shrub, |s| s.category);
    Some(Resolved {
        params,
        category,
        scan_id: c.base.clone(),
    })
}

/// What the season does to a plant, as a cache key: all months that look the same share a class.
pub fn class(p: &flora::PlantParams, month: u8) -> u32 {
    let season = if !p.deciduous {
        4
    } else {
        match month {
            12 | 1 | 2 | 3 if p.marcescent => 5,
            12 | 1 | 2 | 3 => 0,
            4 => 1,
            10 => 2,
            11 if p.marcescent => 6,
            11 => 3,
            _ => 4,
        }
    };
    season | (u32::from(flora::in_bloom(p, month)) << 4)
}

/// (mesh, material, LOD max distance)
type Part = (MeshId, MaterialId, f32);

/// Plants at least this tall (unscaled) are trees: they hand over to their baked impostor early.
const TREE_M: f32 = 3.0;
/// The lawn's tufts never get an impostor: there are tens of thousands of them and the lawn
/// material carries their colour from afar.
pub const LAWN_SPECIES: &str = "flora:lawn-tuft";
/// Species ids of ground clutter (fallen leaves of a tree species): never an impostor either.
pub const CLUTTER_PREFIX: &str = "clutter:";
/// Views around a tree an impostor is baked from.
pub const IMPOSTOR_VIEWS: u32 = 8;
/// Views drawn into an impostor's atlas per frame: a bake spreads over four frames (three of
/// views, one of dilation and mips) instead of one long submit.
const BAKE_VIEWS_PER_FRAME: u32 = 3;
/// Impostors draw up to here at most (metres; beyond, a tree is a few pixels).
pub const IMPOSTOR_MAX_M: f32 = 2500.0;
/// An impostor draws while its plant is taller than about 1.5 pixels: at a 50 degree field of view
/// on a 1080 pixel tall view, `h` metres project to 1.5 pixels at about 770 h metres. Twice that,
/// because the quality's level-of-detail scale (50 % at MEDIUM, 40 % at LOW) shortens it again.
const IMPOSTOR_M_PER_M: f32 = 1540.0;

/// How far the impostor of a plant `h` metres tall draws: by the size it has on screen, not a
/// fixed distance (a 0.6 m perennial reaches about 920 m before the quality's scale, a tree the full
/// `IMPOSTOR_MAX_M`).
pub fn impostor_max(h: f32) -> f32 {
    (IMPOSTOR_M_PER_M * h).clamp(60.0, IMPOSTOR_MAX_M)
}

/// The LOD distances (near, mid, far end) of a plant `h` metres tall, before any impostor:
/// the far mesh of a tree draws to infinity until its impostor is baked, a shrub or ground
/// plant is culled at the end of its far level.
fn lod_distances(h: f32) -> [f32; 3] {
    if h >= TREE_M {
        [8.0 + 3.0 * h, 22.0 + 6.0 * h, f32::INFINITY]
    } else {
        [3.0 + 2.0 * h, 8.0 + 4.0 * h, 15.0 + 12.0 * h]
    }
}

/// A tree with its impostor: its three mesh levels' ends, the last where the impostor takes over.
fn impostor_distances(h: f32) -> [f32; 3] {
    // The far mesh draws to 25 + 3 h: at MEDIUM's half distances a 10 m tree is still a mesh at
    // 27 m, where its impostor no longer shows as large flat cards.
    [4.0 + 1.0 * h, 10.0 + 2.0 * h, 25.0 + 3.0 * h]
}

/// The ends of a plant's `n` mesh levels: the three of `lod_distances` or `impostor_distances`
/// (`baked`), or with the two leaf-level detail levels of HIGH and ULTRA (`flora::generate_levels`)
/// before them: every leaf to 2 + h/2 m (12 m for a 20 m tree, twice that at ULTRA), the larger
/// spray budget to about twice that, then the card levels, ending where they did; or a herb's
/// one stem level (4 levels).
fn level_ends(h: f32, n: usize, baked: bool) -> Vec<f32> {
    match (n, baked && h >= TREE_M) {
        (5, true) => vec![
            2.0 + 0.5 * h,
            4.0 + 1.1 * h,
            6.0 + 1.6 * h,
            10.0 + 2.2 * h,
            25.0 + 3.0 * h,
        ],
        (5, false) => {
            let [a, b, c] = lod_distances(h);
            vec![2.0 + 0.5 * h, 4.0 + 1.1 * h, a.max(6.0 + 1.6 * h), b, c]
        }
        // A herb's or grass's stem level (HIGH and ULTRA) before its three: 3.1 m for a 0.6 m
        // lavender, 5.5 m for a 1.6 m reed grass, twice that at ULTRA and in stills.
        (4, _) => {
            let [a, b, c] = lod_distances(h);
            vec![1.6 + 2.4 * h, a.max(2.6 + 2.6 * h), b, c]
        }
        (_, true) => impostor_distances(h).to_vec(),
        _ => lod_distances(h).to_vec(),
    }
}

/// One variant of a species in one season class.
struct Variant {
    parts: Vec<Part>,
    /// What each part is (bark, foliage...), for re-tinted copies.
    kinds: Vec<flora::Material>,
    /// Trees: (mesh levels shortened for the impostor, the impostor) once baked.
    baked: Option<(Vec<Part>, ImpostorId)>,
    height: f32,
}

struct Tex {
    /// Leaf atlas texels per drawing unit (`Flora::leaf_scale` when made).
    scale: usize,
    atlas: TextureId,
    /// The leaf normal map of a fine atlas (`leaf_scale` above 1).
    atlas_normal: Option<TextureId>,
    bark: TextureId,
    bark_normal: TextureId,
    /// Mean linear colour of the atlas' opaque texels: what a tint of 1 looks like.
    leaf_mean: [f32; 3],
    /// The scanned sets they were made from (`ScanLib::scans`).
    scan_key: String,
}

/// A species' textures at `scale` texels per drawing unit, uploaded (block-compressed where the
/// device can).
fn make_tex(
    r: &mut Renderer,
    params: &flora::PlantParams,
    scale: usize,
    scans: &flora::scan::Scans,
) -> Option<Tex> {
    let t = crate::load_stats::time("plant texture composition", || {
        flora::textures_scanned(params, scale, scans)
    });
    // Block-compressed where the device can (a quarter to an eighth of the memory; the
    // leaf atlas keeps its cutout coverage in every mip), else RGBA8.
    let up = |r: &mut Renderer, i: &flora::Image, srgb: bool, repeat: bool| {
        use visioncraft_render::bc::{BcKind, compress};
        let kind = match (srgb, repeat) {
            (true, false) => BcKind::Rgba, // the leaf atlas
            (true, true) => BcKind::Rgb,   // bark
            (false, _) => BcKind::Normal,
        };
        let space = if srgb {
            visioncraft_render::ColorSpace::Srgb
        } else {
            visioncraft_render::ColorSpace::Linear
        };
        if r.supports_bc()
            && !cfg!(target_arch = "wasm32")
            && let Some(c) = crate::load_stats::time("BC compression (plant)", || {
                compress(&i.rgba, i.width, i.height, kind, space)
            })
            && let Ok(id) = r.add_compressed(std::sync::Arc::new(c), repeat)
        {
            return Some(id);
        }
        scene::upload_texture(r, i.width, i.height, &i.rgba, srgb, repeat)
    };
    (|| {
        Some(Tex {
            scale,
            atlas: up(r, &t.atlas, true, false)?,
            atlas_normal: match &t.atlas_normal {
                Some(n) => Some(up(r, n, false, false)?),
                None => None,
            },
            bark: up(r, &t.bark, true, true)?,
            bark_normal: up(r, &t.bark_normal, false, true)?,
            leaf_mean: opaque_mean(&t.atlas.rgba),
            scan_key: String::new(),
        })
    })()
}

/// Mean linear RGB of the texels with alpha over one half (sRGB RGBA8 input).
pub(crate) fn opaque_mean(rgba: &[u8]) -> [f32; 3] {
    let lin = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (mut sum, mut n) = ([0.0f32; 3], 0u32);
    for px in rgba.as_chunks::<4>().0.iter().filter(|p| p[3] > 128) {
        for k in 0..3 {
            sum[k] += lin(px[k]);
        }
        n += 1;
    }
    sum.map(|v| v / n.max(1) as f32)
}

struct Want {
    id: String,
    class: u32,
    month: u8,
    params: flora::PlantParams,
    /// Where it is planted (a sample of the placements): the nearest goes first.
    at: Vec<glam::Vec3>,
}

/// Placements a want remembers for its urgency (a hedge of a thousand segments needs no more).
const WANT_POINTS: usize = 256;

impl Want {
    /// How urgent it is from `eye` (smaller first): its distance over its height, as large on
    /// screen comes first. Plants placed nowhere in particular (lawn tufts) come after the near.
    fn urgency(&self, eye: glam::Vec3) -> f32 {
        let h = (self.params.height_m * self.params.age.clamp(0.05, 1.0)).max(0.2);
        let d = self
            .at
            .iter()
            .map(|p| p.distance(eye))
            .fold(f32::INFINITY, f32::min);
        if d.is_finite() { d / h } else { 50.0 }
    }
}

/// A species' textures being made off the frame (the pool, the web worker), from which scans,
/// at which scale; the answer is `None` when they could not be made there.
struct TexJob {
    scan_key: String,
    scale: usize,
    answer: Answer,
}

#[derive(Default)]
pub struct Flora {
    textures: HashMap<String, Tex>,
    /// (species, class) to its variants.
    classes: HashMap<(String, u32), Vec<Variant>>,
    queue: Vec<Want>,
    /// Trees whose impostor is still to be baked: (species, class, variant).
    bakes: Vec<(String, u32, u32)>,
    /// Copies with their foliage re-tinted: (species, class, variant, colour bits).
    tinted: HashMap<(String, u32, u32, [u32; 3]), Vec<Part>>,
    /// Species no longer drawn, freed after a few pumps (the scene drops its items first).
    retired: Vec<(String, u8)>,
    /// Counts finished generations, so generators know to rebuild.
    pub rev: u64,
    /// Leaf atlas texels per drawing unit: 2 (512 px cells and a leaf normal map) at HIGH and
    /// ULTRA, else 1. A change makes the textures again (`regenerate`) and the meshes (`pump`:
    /// the detail levels of HIGH and ULTRA come and go with it).
    pub leaf_scale: usize,
    /// Whether the meshes kept were made with the detail levels (`leaf_scale` 2), once any were.
    detail_made: Option<bool>,
    /// The parameters each species' textures were made from, to make them again.
    tex_params: HashMap<String, flora::PlantParams>,
    /// Every plant material made, with what it was made from, to point it at new textures.
    plant_mats: Vec<(String, MaterialId, flora::Material, [f32; 3])>,
    /// Species whose first textures are being made off the frame.
    tex_jobs: HashMap<String, TexJob>,
    /// Species whose textures are being made again (`regenerate`).
    regen_jobs: HashMap<String, TexJob>,
    /// (species, class) whose variants' meshes are being generated, one job per variant.
    meshing: HashMap<(String, u32), Vec<Slot<Generated>>>,
    /// The impostor being baked: (species, class, variant) and the bake.
    baking: Option<((String, u32, u32), visioncraft_render::ImpostorBake)>,
    /// The scanned leaf and bark sets.
    pub scans: crate::scans::ScanLib,
    /// The month plants were last asked for: which leaf set (summer, autumn) textures use.
    month: u8,
    /// Species whose textures wait to be made again (other scans, another atlas scale).
    stale: usize,
    /// When the first species began waiting for its scans.
    scan_wait: Option<f64>,
    /// Nothing was pending at the last pump.
    idle: bool,
}

type Answer = Slot<Option<crate::texture_worker::PlantCompressed>>;

impl Flora {
    pub fn ready(&self, id: &str, class: u32) -> bool {
        self.classes.contains_key(&(id.to_string(), class))
    }

    fn variant(&self, id: &str, class: u32, variant: u32) -> Option<&Variant> {
        self.classes
            .get(&(id.to_string(), class))?
            .get((variant % VARIANTS) as usize)
    }

    /// The LOD parts to draw: a tree's shortened mesh levels once its impostor is there.
    pub fn parts(&self, id: &str, class: u32, variant: u32) -> Option<&[Part]> {
        let v = self.variant(id, class, variant)?;
        Some(v.baked.as_ref().map_or(&v.parts, |b| &b.0))
    }

    /// `parts` with the foliage coloured so that it averages `rgb` (linear): lawn tufts that
    /// match the lawn under them. Materials are made once per colour.
    pub fn tinted(
        &mut self,
        r: &mut Renderer,
        id: &str,
        class: u32,
        variant: u32,
        rgb: [f32; 3],
    ) -> Option<Vec<Part>> {
        let key = (id.to_string(), class, variant, rgb.map(f32::to_bits));
        if let Some(p) = self.tinted.get(&key) {
            return Some(p.clone());
        }
        let tex = self.textures.get(id)?;
        let v = self.variant(id, class, variant)?;
        let tint = [0, 1, 2].map(|k| (rgb[k] / tex.leaf_mean[k].max(1e-3)).clamp(0.0, 4.0));
        let mut foliage: Option<MaterialId> = None;
        let mut out = Vec::new();
        for (&(m, mat, d), &kind) in v.parts.iter().zip(&v.kinds) {
            let mat = if kind == flora::Material::Foliage {
                *foliage.get_or_insert_with(|| {
                    scene::plant_material(
                        r,
                        kind,
                        (tex.atlas, tex.atlas_normal, tex.bark, tex.bark_normal),
                        tint,
                    )
                })
            } else {
                mat
            };
            out.push((m, mat, d));
        }
        if let Some(m) = foliage {
            self.plant_mats
                .push((id.to_string(), m, flora::Material::Foliage, tint));
        }
        self.tinted.insert(key, out.clone());
        Some(out)
    }

    /// The impostor that follows `parts`' last level, once baked, and how far it draws.
    /// Smaller plants share the impostor of their first variant (a few pixels tall, the
    /// variants look alike; one atlas per species instead of three).
    /// (impostor, how far it draws, whether the plant is smaller than a tree).
    pub fn impostor(&self, id: &str, class: u32, variant: u32) -> Option<(ImpostorId, f32, bool)> {
        let v = self.variant(id, class, variant)?;
        let baked = v.baked.as_ref().or_else(|| {
            (v.height < TREE_M)
                .then(|| self.variant(id, class, 0))
                .flatten()
                .and_then(|first| first.baked.as_ref())
        })?;
        Some((baked.1, impostor_max(v.height), v.height < TREE_M))
    }

    /// Bakes one waiting impostor (from the far mesh level). True when one was made.
    fn bake_one(&mut self, r: &mut Renderer) -> bool {
        // A bake runs over several frames, a few views a frame (`BAKE_VIEWS_PER_FRAME`).
        while self.baking.is_none() {
            let Some(key) = self.bakes.pop() else {
                return false;
            };
            let Some(v) = self
                .classes
                .get(&(key.0.clone(), key.1))
                .and_then(|c| c.get(key.2 as usize))
            else {
                continue;
            };
            // From the nearest card level, not the far one: the impostor is drawn once, and the
            // far level's few cards (its triangle budget is for every frame) baked a crown of
            // bare limbs with a few tufts, and cast them as long arcs in its shadow.
            let mut ends: Vec<f32> = v.parts.iter().map(|p| p.2).collect();
            ends.sort_by(f32::total_cmp);
            ends.dedup();
            let Some(&src_end) = ends.get(ends.len().saturating_sub(3)) else {
                continue;
            };
            let src: Vec<(MeshId, MaterialId)> = v
                .parts
                .iter()
                .filter(|p| p.2 == src_end)
                .map(|p| (p.0, p.1))
                .collect();
            if let Ok(bake) = r.start_impostor_bake(&src, None, IMPOSTOR_VIEWS) {
                self.baking = Some((key, bake));
            } // else it stays a mesh to infinity
        }
        let Some((_, bake)) = self.baking.as_mut() else {
            return false;
        };
        let result = crate::load_stats::time("impostor bake step (main thread)", || {
            r.impostor_bake_step(bake, BAKE_VIEWS_PER_FRAME)
        });
        let Some(result) = result else {
            return false;
        };
        let Some(((id, class, variant), _)) = self.baking.take() else {
            return false;
        };
        let Ok(imp) = result else {
            return false; // a part went away: stays a mesh
        };
        let Some(v) = self
            .classes
            .get_mut(&(id, class))
            .and_then(|c| c.get_mut(variant as usize))
        else {
            r.remove_impostor(imp);
            return false;
        };
        crate::load_stats::add("impostors baked", 0.0);
        let far = v.parts.iter().map(|p| p.2).fold(0.0, f32::max);
        // Trees hand over early; smaller plants keep their meshes as far as before and
        // then become an impostor instead of disappearing.
        let mut levels: Vec<f32> = v.parts.iter().map(|p| p.2).collect();
        levels.sort_by(f32::total_cmp);
        levels.dedup();
        let ends = level_ends(v.height, levels.len(), true);
        let short = v
            .parts
            .iter()
            .map(|&(m, mat, d)| {
                let rank = levels.iter().position(|l| *l == d).unwrap_or(0);
                // the last level is always the impostor's hand-over
                let rank = if d == far {
                    ends.len() - 1
                } else {
                    rank.min(ends.len() - 2)
                };
                (m, mat, ends[rank])
            })
            .collect();
        // A rebake (another impostor resolution) frees the atlas it replaces.
        if let Some((_, old)) = v.baked.replace((short, imp)) {
            r.remove_impostor(old);
        }
        self.rev += 1;
        true
    }

    /// Bakes every impostor again, at the quality's impostor resolution now set.
    pub fn rebake(&mut self) {
        for ((id, class), variants) in &self.classes {
            for (v, variant) in variants.iter().enumerate() {
                let key = (id.clone(), *class, v as u32);
                if variant.baked.is_some() && !self.bakes.contains(&key) {
                    self.bakes.push(key);
                }
            }
        }
    }

    /// Asks for species `id` in season `class`, planted `at` (world positions, may be empty);
    /// `scan_id` names the built-in species whose scans it takes (a custom species: its base).
    pub fn want(
        &mut self,
        id: &str,
        scan_id: Option<&str>,
        class: u32,
        month: u8,
        params: &flora::PlantParams,
        at: &[glam::Vec3],
    ) {
        self.month = month;
        if let Some(s) = scan_id {
            self.scans.alias(id, s);
        }
        if self.ready(id, class) {
            return;
        }
        let room = |w: &Want| WANT_POINTS.saturating_sub(w.at.len());
        match self
            .queue
            .iter_mut()
            .find(|w| w.id == id && w.class == class)
        {
            Some(w) => {
                let n = room(w).min(at.len());
                w.at.extend_from_slice(&at[..n]);
            }
            None => self.queue.push(Want {
                id: id.into(),
                class,
                month,
                params: params.clone(),
                at: at[..at.len().min(WANT_POINTS)].to_vec(),
            }),
        }
    }

    /// Generations and impostor bakes still to do.
    pub fn pending(&self) -> usize {
        self.queue.len()
            + self.bakes.len()
            + usize::from(self.baking.is_some())
            + self.stale
            + self.scans.loading()
    }

    /// Up to `n` species still being made (for a message), most urgent first as last pumped.
    pub fn waiting_names(&self, n: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for w in &self.queue {
            if out.len() < n && !out.contains(&w.id) {
                out.push(w.id.clone());
            }
        }
        out
    }

    /// Frees species `id` (a species editor preview that changed) once nothing draws it.
    pub fn retire(&mut self, id: &str) {
        self.queue.retain(|w| w.id != id);
        // shortcut: the meshes of variants already uploaded for a retired job stay on the GPU
        // until the app closes; free them here if species previews get edited a lot
        // Jobs on their way finish; their results are dropped.
        self.meshing.retain(|k, _| k.0 != id);
        self.tex_jobs.remove(id);
        self.regen_jobs.remove(id);
        if self.baking.as_ref().is_some_and(|b| b.0.0 == id) {
            self.baking = None;
        }
        self.bakes.retain(|b| b.0 != id);
        self.retired.push((id.to_string(), 3));
    }

    fn free_retired(&mut self, r: &mut Renderer) {
        for e in &mut self.retired {
            e.1 = e.1.saturating_sub(1);
        }
        let due: Vec<String> = self
            .retired
            .iter()
            .filter(|e| e.1 == 0)
            .map(|e| e.0.clone())
            .collect();
        self.retired.retain(|e| e.1 > 0);
        for id in due {
            let keys: Vec<(String, u32)> =
                self.classes.keys().filter(|k| k.0 == id).cloned().collect();
            for k in keys {
                for v in self.classes.remove(&k).unwrap_or_default() {
                    let mut mats: Vec<MaterialId> = Vec::new();
                    for (m, mat, _) in v.parts {
                        r.remove_mesh(m);
                        if !mats.contains(&mat) {
                            mats.push(mat);
                        }
                    }
                    mats.into_iter().for_each(|m| r.remove_material(m));
                    if let Some((_, imp)) = v.baked {
                        r.remove_impostor(imp);
                    }
                }
            }
            self.tinted.retain(|k, _| k.0 != id);
            self.plant_mats.retain(|e| e.0 != id);
            self.tex_params.remove(&id);
            if let Some(t) = self.textures.remove(&id) {
                for tex in [t.atlas, t.bark, t.bark_normal]
                    .into_iter()
                    .chain(t.atlas_normal)
                {
                    r.remove_texture(tex);
                }
            }
        }
    }

    /// GPU memory of the species' textures (leaf atlases, their normal maps, bark).
    pub fn texture_bytes(&self, r: &Renderer) -> u64 {
        self.textures
            .values()
            .flat_map(|t| {
                [
                    Some(t.atlas),
                    t.atlas_normal,
                    Some(t.bark),
                    Some(t.bark_normal),
                ]
            })
            .flatten()
            .map(|id| r.texture_bytes(id))
            .sum()
    }

    /// Makes species' textures again at the current `leaf_scale` when they were made at another
    /// (the quality changed) or from other scans (they arrived): off the frame, several at once;
    /// each species' materials point at its new ones once there, the old are freed and the
    /// impostors baked again. True when one was done.
    fn regenerate(&mut self, r: &mut Renderer) -> bool {
        let scale = self.leaf_scale.max(1);
        let month = self.month;
        let mut stale = Vec::new();
        for (id, t) in &self.textures {
            let key = match self.tex_params.get(id) {
                Some(p) => self.scans.scans(id, month, p).1,
                None => t.scan_key.clone(),
            };
            if t.scale != scale || t.scan_key != key {
                stale.push(id.clone());
            }
        }
        self.stale = stale.len();
        let mut done = false;
        for id in stale {
            let Some(params) = self.tex_params.get(&id).cloned() else {
                if let Some(t) = self.textures.get_mut(&id) {
                    t.scale = scale;
                }
                continue;
            };
            let (scans, key) = self.scans.scans(&id, month, &params);
            let new = match self.regen_jobs.get(&id) {
                // Made from what is wanted now: take it once there.
                Some(j) if j.scale == scale && j.scan_key == key => match take(&j.answer) {
                    None => continue,
                    Some(got) => {
                        self.regen_jobs.remove(&id);
                        upload_tex(r, got, scale, key.clone())
                            .or_else(|| make_tex(r, &params, scale, &scans))
                    }
                },
                _ if r.supports_bc() => {
                    if work_pool::backlog() < 2 * work_pool::size() {
                        let job = start_tex(&params, scale, &scans, &key, 0.0);
                        self.regen_jobs.insert(id, job);
                    }
                    continue;
                }
                _ => make_tex(r, &params, scale, &scans),
            };
            let Some(new) = new.map(|t| Tex {
                scan_key: key.clone(),
                ..t
            }) else {
                // Cannot make them (the GPU refused): keep the old ones.
                if let Some(t) = self.textures.get_mut(&id) {
                    t.scale = scale;
                    t.scan_key = key;
                }
                continue;
            };
            for (_, m, kind, tint) in self.plant_mats.iter().filter(|e| e.0 == id) {
                r.set_material(
                    *m,
                    &scene::plant_material_desc(
                        *kind,
                        (new.atlas, new.atlas_normal, new.bark, new.bark_normal),
                        *tint,
                    ),
                );
            }
            if let Some(old) = self.textures.insert(id, new) {
                for t in [old.atlas, old.bark, old.bark_normal]
                    .into_iter()
                    .chain(old.atlas_normal)
                {
                    r.remove_texture(t);
                }
            }
            done = true;
            if !r.supports_bc() {
                break; // made here: one a frame
            }
        }
        if done {
            self.rebake();
            self.rev += 1;
        }
        done
    }

    /// Frees every species' meshes, impostors and materials (not their textures): the scene
    /// asks for them again on its sync after this pump (`rev`), and they are made with the
    /// levels the quality now draws. Runs before that sync, so nothing drawn still points at them.
    fn remesh(&mut self, r: &mut Renderer) {
        self.bakes.clear();
        self.baking = None;
        self.meshing.clear();
        self.tinted.clear();
        for (_, variants) in self.classes.drain() {
            for v in variants {
                v.parts.iter().for_each(|p| r.remove_mesh(p.0));
                if let Some((_, imp)) = v.baked {
                    r.remove_impostor(imp);
                }
            }
        }
        // Every plant material, the tinted copies' too, is made per species and class.
        for (_, m, _, _) in self.plant_mats.drain(..) {
            r.remove_material(m);
        }
        self.rev += 1;
    }

    /// How many species (with their seasons) are generated and kept.
    pub fn kept(&self) -> usize {
        self.classes.len()
    }

    /// One frame's step: hands the most urgent waiting work (nearest and largest from `eye`
    /// first) to the pool, uploads what has arrived (within a few milliseconds), and steps an
    /// impostor bake. True when a species became ready (the scene must sync again).
    pub fn pump(&mut self, r: &mut Renderer, eye: glam::Vec3) -> bool {
        if !self.retired.is_empty() {
            self.free_retired(r);
        }
        let detail = self.leaf_scale >= 2;
        if self.detail_made.replace(detail).is_some_and(|d| d != detail) {
            self.remesh(r);
        }
        let mut finished = self.regenerate(r);
        // Textures that arrived.
        let arrived: Vec<String> = self
            .tex_jobs
            .iter()
            .filter(|(_, j)| j.answer.lock().is_ok_and(|a| a.is_some()))
            .map(|(id, _)| id.clone())
            .collect();
        for id in arrived {
            let Some(job) = self.tex_jobs.remove(&id) else {
                continue;
            };
            let Some(params) = self
                .queue
                .iter()
                .find(|w| w.id == id)
                .map(|w| w.params.clone())
            else {
                continue; // nobody wants it any more
            };
            let got = take(&job.answer).flatten();
            let tex = upload_tex(r, got, job.scale, job.scan_key.clone()).or_else(|| {
                // Not made off the frame (no worker, the GPU refused): here.
                let (scans, _) = self.scans.scans(&id, self.month, &params);
                make_tex(r, &params, job.scale, &scans)
            });
            self.tex_params.insert(id.clone(), params);
            match tex {
                Some(t) => {
                    let t = Tex {
                        scan_key: job.scan_key,
                        ..t
                    };
                    self.textures.insert(id, t);
                }
                // stays a placeholder
                None => self.queue.retain(|w| w.id != id),
            }
        }
        // The most urgent first.
        self.queue
            .sort_by(|a, b| a.urgency(eye).total_cmp(&b.urgency(eye)));
        // Natively the pool is kept a little ahead of its threads; on the web a job runs at once
        // on the page, so one species' meshes a frame.
        let mut meshes_started = 0;
        let room = |extra: usize, started: usize| {
            if cfg!(target_arch = "wasm32") {
                started == 0
            } else {
                work_pool::backlog() + extra <= 2 * work_pool::size()
            }
        };
        let budget = crate::assets::Budget::new(6.0);
        let mut i = 0;
        while i < self.queue.len() {
            let w = &self.queue[i];
            let (id, class, month, urgency) = (w.id.clone(), w.class, w.month, w.urgency(eye));
            let params = w.params.clone();
            i += 1;
            if !self.textures.contains_key(&id) && !self.tex_jobs.contains_key(&id) {
                let scale = self.leaf_scale.max(1);
                let (scans, key) = self.scans.scans(&id, month, &params);
                // Natively the scans come off the disk in a moment: wait for them (ten seconds
                // at most, for a machine busy with other work) instead of making the textures
                // twice, and of caching textures from a set that was only half there. On the web
                // the drawn ones show first.
                let since = *self.scan_wait.get_or_insert_with(crate::assets::now_ms);
                let waiting = cfg!(not(target_arch = "wasm32"))
                    && self.scans.arriving(&id, month, &params)
                    && crate::assets::now_ms() - since < 10_000.0;
                if !waiting && r.supports_bc() && (cfg!(target_arch = "wasm32") || room(1, 0)) {
                    let job = start_tex(&params, scale, &scans, &key, urgency);
                    self.tex_jobs.insert(id.clone(), job);
                } else if !waiting && !r.supports_bc() && !budget.spent() {
                    // Without block compression they are plain pictures: made here.
                    let tex =
                        make_tex(r, &params, scale, &scans).map(|t| Tex { scan_key: key, ..t });
                    self.tex_params.insert(id.clone(), params.clone());
                    match tex {
                        Some(t) => {
                            self.textures.insert(id.clone(), t);
                        }
                        None => {
                            i -= 1;
                            self.queue.remove(i);
                            continue;
                        }
                    }
                }
            }
            let k = (id.clone(), class);
            if !self.meshing.contains_key(&k) {
                if !room(VARIANTS as usize, meshes_started) {
                    continue;
                }
                meshes_started += 1;
                let slots = (0..VARIANTS)
                    .map(|v| {
                        let (name, params) = (id.clone(), params.clone());
                        // Variants of one species right after one another.
                        work_pool::run(urgency + 1e-3 * v as f32, move || {
                            generate_variant(&name, &params, v, month, detail)
                        })
                    })
                    .collect();
                self.meshing.insert(k.clone(), slots);
            }
            let ready = self.textures.contains_key(&id)
                && self.meshing[&k]
                    .iter()
                    .all(|s| s.lock().is_ok_and(|g| g.is_some()));
            // At least one species a frame; more while the frame's few milliseconds last.
            if ready && (!finished || !budget.spent()) {
                let slots = self.meshing.remove(&k).unwrap_or_default();
                let generated: Vec<Generated> = slots.iter().filter_map(take).collect();
                // Started before the quality changed (a still's quality is set as the project's
                // plants begin): made again with the levels the quality draws now.
                if generated.iter().any(|g| g.detail != detail) {
                    continue;
                }
                i -= 1;
                self.queue.remove(i);
                self.finish(r, &id, class, generated);
                finished = true;
            }
        }
        // Bakes run beside the generation: a far tree turns into its impostor without waiting
        // for every other species. Several steps a frame while its few milliseconds last.
        let bake_budget = crate::assets::Budget::new(3.0);
        let mut baked = false;
        loop {
            baked |= self.bake_one(r);
            if bake_budget.spent() || (self.baking.is_none() && self.bakes.is_empty()) {
                break;
            }
        }
        // Everything done: once more, for what waits for the whole set (the ground clutter).
        let idle = self.pending() == 0;
        let settled = idle && !std::mem::replace(&mut self.idle, idle);
        if finished || settled {
            self.rev += 1;
        }
        finished || baked || settled
    }

    /// Uploads a species' generated variants in one season class and asks for their impostors.
    fn finish(&mut self, r: &mut Renderer, id: &str, class: u32, generated: Vec<Generated>) {
        let upload_t = crate::assets::now_ms();
        // Crowns get their depth occlusion: trees and shrubs, not clipped hedges (their outside
        // is all surface) or low plants (darkened, their impostors read as grey blobs far off).
        let crowned = !matches!(
            self.tex_params.get(id).map(|p| &p.form),
            Some(flora::Form::Clipped(_)) | None
        );
        let hedge = matches!(
            self.tex_params.get(id).map(|p| &p.form),
            Some(flora::Form::Clipped(_))
        );
        let tex = &self.textures[id];
        let mut variants = Vec::new();
        let mut made = Vec::new();
        for generated in generated {
            let mut parts = Vec::new();
            let mut kinds = Vec::new();
            let mut mats: HashMap<usize, MaterialId> = HashMap::new();
            let height = generated.height;
            let ends = level_ends(height, generated.levels.len(), false);
            for (li, split, tint) in generated.levels {
                let dist = ends[li];
                // Crowns of plants, not lawn tufts or fallen leaves lying flat: the leaves and
                // the branches among them.
                let crown = (crowned && height >= 1.5 && !id.starts_with(CLUTTER_PREFIX))
                    .then(|| split.iter().find(|(k, _)| *k == flora::Material::Foliage))
                    .flatten()
                    .map(|(_, p)| scene::part_bounds(&p.positions));
                for (kind, part) in split {
                    let wood = kind != flora::Material::Foliage;
                    let Some(m) = scene::upload_plant_mesh(r, &part, crown, wood, hedge) else {
                        continue;
                    };
                    let mat = *mats.entry(kind as usize).or_insert_with(|| {
                        let m = scene::plant_material(
                            r,
                            kind,
                            (tex.atlas, tex.atlas_normal, tex.bark, tex.bark_normal),
                            tint,
                        );
                        made.push((m, kind, tint));
                        m
                    });
                    parts.push((m, mat, dist));
                    kinds.push(kind);
                }
            }
            variants.push(Variant {
                parts,
                kinds,
                baked: None,
                height,
            });
        }
        for (v, variant) in variants.iter().enumerate() {
            // Lawn tufts and ground clutter are culled near: no impostor.
            let near_only = id == LAWN_SPECIES || id.starts_with(CLUTTER_PREFIX);
            if !near_only && (variant.height >= TREE_M || v == 0) {
                self.bakes.push((id.to_string(), class, v as u32));
            }
        }
        for (m, kind, tint) in made {
            self.plant_mats.push((id.to_string(), m, kind, tint));
        }
        self.classes.insert((id.to_string(), class), variants);
        crate::load_stats::add(
            "plant mesh upload (main thread)",
            crate::assets::now_ms() - upload_t,
        );
    }
}

/// Starts making a species' textures off the frame: natively on the pool (through the disk
/// cache), on the web in the texture worker.
fn start_tex(
    params: &flora::PlantParams,
    scale: usize,
    scans: &flora::scan::Scans,
    scan_key: &str,
    urgency: f32,
) -> TexJob {
    let (params, scans) = (params.clone(), scans.clone());
    #[cfg(target_arch = "wasm32")]
    let answer = {
        let _ = urgency;
        let answer = Answer::default();
        let slot = answer.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let got = crate::texture_worker::plant(&params, scale, &scans).await;
            if let Ok(mut a) = slot.lock() {
                *a = Some(got);
            }
        });
        answer
    };
    #[cfg(not(target_arch = "wasm32"))]
    let answer = work_pool::run(urgency, move || {
        let json = serde_json::to_vec(&params).unwrap_or_default();
        // The scans' bytes, not their names: a scan file edited in place is another key.
        let key = crate::plant_cache::key(&[
            b"plant textures",
            &json,
            &(scale as u32).to_le_bytes(),
            &crate::plant_cache::digest(&scans.pack()),
        ]);
        crate::plant_cache::textures(key, || {
            crate::texture_worker::plant_compressed(&params, scale, &scans)
        })
    });
    TexJob {
        scan_key: scan_key.to_string(),
        scale,
        answer,
    }
}

/// A species' textures made off the frame, uploaded; `None` when they were not made there or
/// the GPU refused them.
fn upload_tex(
    r: &mut Renderer,
    got: Option<crate::texture_worker::PlantCompressed>,
    scale: usize,
    scan_key: String,
) -> Option<Tex> {
    let (leaf_mean, images) = got?;
    let mut ids = Vec::new();
    for (c, repeat) in images {
        match r.add_compressed(c, repeat) {
            Ok(t) => ids.push(t),
            Err(_) => {
                ids.into_iter().for_each(|t| r.remove_texture(t));
                return None;
            }
        }
    }
    let (atlas, atlas_normal, bark, bark_normal) = match ids[..] {
        [a, n, b, bn] => (a, Some(n), b, bn),
        [a, b, bn] => (a, None, b, bn),
        _ => {
            ids.into_iter().for_each(|t| r.remove_texture(t));
            return None;
        }
    };
    Some(Tex {
        scale,
        atlas,
        atlas_normal,
        bark,
        bark_normal,
        leaf_mean,
        scan_key,
    })
}

/// A level's meshes, one per material.
type Parts = Vec<(flora::Material, flora::MeshPart)>;

/// One variant's meshes at every level, generated away from the renderer.
struct Generated {
    height: f32,
    /// (level, parts by material, foliage tint)
    levels: Vec<(usize, Parts, [f32; 3])>,
    /// Made with the detail levels of HIGH and ULTRA.
    detail: bool,
}

/// `generate_variant_now`, natively through the disk cache (`plant_cache`).
fn generate_variant(
    name: &str,
    params: &flora::PlantParams,
    v: u32,
    month: u8,
    detail: bool,
) -> Generated {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let json = serde_json::to_vec(params).unwrap_or_default();
        let key = crate::plant_cache::key(&[&json, &v.to_le_bytes(), &[month, detail as u8]]);
        let (height, levels) = crate::plant_cache::meshes(key, || {
            let g = generate_variant_now(name, params, v, month, detail);
            (g.height, g.levels)
        });
        Generated {
            height,
            levels,
            detail,
        }
    }
    #[cfg(target_arch = "wasm32")]
    generate_variant_now(name, params, v, month, detail)
}

/// Every level of one variant, nearest first; with `detail` (HIGH and ULTRA) trees get the
/// leaf-level levels before the card levels.
fn generate_variant_now(
    name: &str,
    params: &flora::PlantParams,
    v: u32,
    month: u8,
    detail: bool,
) -> Generated {
    let t = crate::assets::now_ms();
    let meshes = flora::generate_levels(
        params,
        u64::from(v) + 1,
        Some(flora::Season { month }),
        detail,
    );
    // The height of the card levels' near mesh, as before the detail levels came.
    let height = meshes[meshes.len() - 3].bounds().1[1].max(0.2);
    let levels = meshes
        .iter()
        .enumerate()
        .map(|(li, m)| (li, m.split_parts(), m.foliage_tint))
        .collect();
    crate::load_stats::add(&format!("~plant mesh {name}"), crate::assets::now_ms() - t);
    Generated {
        height,
        levels,
        detail,
    }
}

pub(crate) use crate::work_pool::Slot;
use crate::work_pool::{self, take};

/// Runs `f` off the frame (the pool; on the web, where there are no threads, at once); its
/// result lands in the slot.
pub(crate) fn spawn<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Slot<T> {
    work_pool::run(0.0, f)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A herb's stem level ends before its near card level; the mid and far levels end where
    /// they did without it.
    #[test]
    fn a_herbs_stem_level_comes_first() {
        for h in [0.25, 0.6, 1.6] {
            let four = level_ends(h, 4, false);
            let three = level_ends(h, 3, false);
            assert!(four.windows(2).all(|w| w[0] < w[1]), "{four:?}");
            assert!(four[0] >= 2.0 && four[0] < three[0].max(four[1]));
            assert_eq!(four[2..], three[1..]);
        }
    }

    #[test]
    fn summer_months_share_a_class_and_winter_differs() {
        let p = flora::Species::by_id("beech")
            .or_else(|| {
                flora::species()
                    .iter()
                    .find(|s| s.params.deciduous && s.params.bloom_months.is_empty())
            })
            .unwrap()
            .params
            .clone();
        assert_eq!(class(&p, 6), class(&p, 7));
        assert_ne!(class(&p, 1), class(&p, 6));
        assert_eq!(class(&p, 1), class(&p, 2));
    }

    /// A load comes out the same however the pool schedules it: every variant's meshes and a
    /// species' compressed textures, made on the pool all at once, are byte-equal to making
    /// them one after another here.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn parallel_generation_is_byte_equal_to_serial() {
        let species: Vec<_> = flora::species().iter().take(3).collect();
        let bytes = |g: &Generated| crate::plant_cache::encode_levels(g.height, &g.levels);
        let serial: Vec<Vec<u8>> = species
            .iter()
            .flat_map(|s| {
                (0..VARIANTS).map(|v| bytes(&generate_variant_now("t", &s.params, v, 6, false)))
            })
            .collect();
        let slots: Vec<_> = species
            .iter()
            .flat_map(|s| {
                (0..VARIANTS).map(|v| {
                    let p = s.params.clone();
                    work_pool::run((VARIANTS - v) as f32, move || {
                        generate_variant_now("t", &p, v, 6, false)
                    })
                })
            })
            .collect();
        fn wait<T>(s: &Slot<T>) -> T {
            loop {
                if let Some(v) = take(s) {
                    break v;
                }
                std::thread::yield_now();
            }
        }
        let parallel: Vec<Vec<u8>> = slots.iter().map(|s| bytes(&wait(s))).collect();
        assert!(
            serial == parallel,
            "parallel meshes differ from serial ones"
        );

        let p = species[0].params.clone();
        let scans = flora::scan::Scans::default();
        let here = crate::texture_worker::plant_compressed(&p, 1, &scans).unwrap();
        let s2 = scans.clone();
        let pooled = wait(&work_pool::run(0.0, move || {
            crate::texture_worker::plant_compressed(&p, 1, &s2)
        }))
        .unwrap();
        assert_eq!(here.0, pooled.0);
        assert_eq!(here.1.len(), pooled.1.len());
        for (a, b) in here.1.iter().zip(&pooled.1) {
            assert!(a.0.levels == b.0.levels && a.1 == b.1, "textures differ");
        }
    }

    #[test]
    fn the_nearest_and_largest_come_first() {
        let p = flora::species()[0].params.clone();
        let want = |at: Vec<glam::Vec3>, h: f32| Want {
            id: "x".into(),
            class: 0,
            month: 6,
            params: flora::PlantParams {
                height_m: h,
                age: 1.0,
                ..p.clone()
            },
            at,
        };
        let eye = glam::Vec3::ZERO;
        let near = want(vec![glam::Vec3::new(5.0, 0.0, 0.0)], 2.0);
        let far = want(vec![glam::Vec3::new(50.0, 0.0, 0.0)], 2.0);
        let far_tree = want(vec![glam::Vec3::new(50.0, 0.0, 0.0)], 30.0);
        assert!(near.urgency(eye) < far.urgency(eye));
        assert!(far_tree.urgency(eye) < far.urgency(eye));
        // nearest of several placements counts
        let both = want(
            vec![
                glam::Vec3::new(50.0, 0.0, 0.0),
                glam::Vec3::new(5.0, 0.0, 0.0),
            ],
            2.0,
        );
        assert_eq!(both.urgency(eye), near.urgency(eye));
    }

    #[test]
    fn flowering_changes_the_class() {
        let mut p = flora::species()[0].params.clone();
        p.deciduous = false;
        p.bloom_months = vec![5];
        assert_ne!(class(&p, 5), class(&p, 6));
        assert_eq!(class(&p, 6), class(&p, 7));
    }
}
