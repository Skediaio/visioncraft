//! Real plants from `visioncraft-flora`: generated lazily (textures of one species, or the meshes
//! of one species and season class, per frame), uploaded once, drawn as LOD items.

use crate::scene::{self, MaterialId, MeshId};
use std::collections::HashMap;
use visioncraft_core::Project;
use visioncraft_flora as flora;
use visioncraft_render::{ImpostorId, Renderer, TextureId};

pub const VARIANTS: u32 = 3;

pub struct Resolved {
    pub params: flora::PlantParams,
    pub category: flora::Category,
}

/// The generator parameters of a palette asset id: `flora:<id>` or a custom species of the project.
pub fn resolve(project: &Project, asset: &str) -> Option<Resolved> {
    if let Some(id) = asset.strip_prefix("flora:") {
        let s = flora::Species::by_id(id)?;
        return Some(Resolved {
            params: s.params.clone(),
            category: s.category,
        });
    }
    let c = project.species.iter().find(|c| c.id == asset)?;
    let params: flora::PlantParams = serde_json::from_value(c.params.clone()).ok()?;
    let category = c
        .base
        .as_deref()
        .and_then(flora::Species::by_id)
        .map_or(flora::Category::Shrub, |s| s.category);
    Some(Resolved { params, category })
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
    season | (u32::from(p.bloom_months.contains(&month)) << 4)
}

/// (mesh, material, LOD max distance)
type Part = (MeshId, MaterialId, f32);

/// Plants at least this tall (unscaled) are trees: they hand over to their baked impostor early.
const TREE_M: f32 = 3.0;
/// The lawn's tufts never get an impostor: there are tens of thousands of them and the lawn
/// material carries their colour from afar.
pub const LAWN_SPECIES: &str = "flora:lawn-tuft";
/// Views around a tree an impostor is baked from.
const IMPOSTOR_VIEWS: u32 = 8;
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
    [4.0 + 1.0 * h, 8.0 + 1.5 * h, 15.0 + 2.0 * h]
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
}

/// A species' textures at `scale` texels per drawing unit, uploaded (block-compressed where the
/// device can).
fn make_tex(r: &mut Renderer, params: &flora::PlantParams, scale: usize) -> Option<Tex> {
    let t = flora::textures_at(params, scale);
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
            && let Some(c) = compress(&i.rgba, i.width, i.height, kind, space)
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
        })
    })()
}

/// Mean linear RGB of the texels with alpha over one half (sRGB RGBA8 input).
fn opaque_mean(rgba: &[u8]) -> [f32; 3] {
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
    /// Leaf atlas texels per drawing unit for species generated from now on: 2 (512 px cells and
    /// a leaf normal map) at HIGH and ULTRA, else 1.
    /// shortcut: species already generated keep their atlas when the quality changes; regenerate
    /// on a change if Auto's steps make the difference visible
    pub leaf_scale: usize,
    /// The parameters each species' textures were made from, to make them again.
    tex_params: HashMap<String, flora::PlantParams>,
    /// Every plant material made, with what it was made from, to point it at new textures.
    plant_mats: Vec<(String, MaterialId, flora::Material, [f32; 3])>,
}

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
        while let Some((id, class, variant)) = self.bakes.pop() {
            let Some(v) = self
                .classes
                .get_mut(&(id, class))
                .and_then(|c| c.get_mut(variant as usize))
            else {
                continue;
            };
            let far = v.parts.iter().map(|p| p.2).fold(0.0, f32::max);
            let src: Vec<(MeshId, MaterialId)> = v
                .parts
                .iter()
                .filter(|p| p.2 == far)
                .map(|p| (p.0, p.1))
                .collect();
            let Ok(imp) = r.bake_impostor(&src, None, IMPOSTOR_VIEWS) else {
                continue; // stays a mesh to infinity
            };
            // Trees hand over early; smaller plants keep their meshes as far as before and
            // then become an impostor instead of disappearing.
            let ends = if v.height >= TREE_M {
                impostor_distances(v.height)
            } else {
                lod_distances(v.height)
            };
            let mut levels: Vec<f32> = v.parts.iter().map(|p| p.2).collect();
            levels.sort_by(f32::total_cmp);
            levels.dedup();
            let short = v
                .parts
                .iter()
                .map(|&(m, mat, d)| {
                    let rank = levels.iter().position(|l| *l == d).unwrap_or(0);
                    // the last level is always the impostor's hand-over
                    let rank = if d == far { 2 } else { rank.min(1) };
                    (m, mat, ends[rank])
                })
                .collect();
            // A rebake (another impostor resolution) frees the atlas it replaces.
            if let Some((_, old)) = v.baked.replace((short, imp)) {
                r.remove_impostor(old);
            }
            self.rev += 1;
            return true;
        }
        false
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

    pub fn want(&mut self, id: &str, class: u32, month: u8, params: &flora::PlantParams) {
        if !self.ready(id, class) && !self.queue.iter().any(|w| w.id == id && w.class == class) {
            self.queue.push(Want {
                id: id.into(),
                class,
                month,
                params: params.clone(),
            });
        }
    }

    /// Generations and impostor bakes still to do.
    pub fn pending(&self) -> usize {
        self.queue.len() + self.bakes.len()
    }

    /// Frees species `id` (a species editor preview that changed) once nothing draws it.
    pub fn retire(&mut self, id: &str) {
        self.queue.retain(|w| w.id != id);
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

    /// Makes one species' textures again at the current `leaf_scale` when they were made at
    /// another (the quality changed): its materials point at the new ones, the old are freed
    /// and the impostors baked again. True when one was done.
    fn regenerate_one(&mut self, r: &mut Renderer) -> bool {
        let scale = self.leaf_scale.max(1);
        let Some(id) = self
            .textures
            .iter()
            .find(|(_, t)| t.scale != scale)
            .map(|(id, _)| id.clone())
        else {
            return false;
        };
        let Some(new) = self.tex_params.get(&id).and_then(|p| make_tex(r, p, scale)) else {
            // Cannot make them (no parameters, or the GPU refused): keep the old ones.
            if let Some(t) = self.textures.get_mut(&id) {
                t.scale = scale;
            }
            return false;
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
        self.rebake();
        self.rev += 1;
        true
    }

    /// How many species (with their seasons) are generated and kept.
    pub fn kept(&self) -> usize {
        self.classes.len()
    }

    /// One step: the textures of the first wanted species, or its meshes. True when something finished.
    #[allow(clippy::map_entry)] // the textures are generated between the check and the insert
    pub fn pump(&mut self, r: &mut Renderer) -> bool {
        if !self.retired.is_empty() {
            self.free_retired(r);
        }
        if self.regenerate_one(r) {
            return true;
        }
        let Some(w) = self.queue.first() else {
            return self.bake_one(r);
        };
        let (id, class, month) = (w.id.clone(), w.class, w.month);
        if !self.textures.contains_key(&id) {
            let scale = self.leaf_scale.max(1);
            let tex = make_tex(r, &w.params, scale);
            self.tex_params.insert(id.clone(), w.params.clone());
            match tex {
                Some(tex) => {
                    self.textures.insert(id, tex);
                }
                None => {
                    self.queue.remove(0); // stays a placeholder
                }
            }
            return false;
        }
        let w = self.queue.remove(0);
        let tex = &self.textures[&id];
        let mut made = Vec::new();
        let mut variants = Vec::new();
        for v in 0..VARIANTS {
            let mut parts = Vec::new();
            let mut kinds = Vec::new();
            let mut mats: HashMap<usize, MaterialId> = HashMap::new();
            let mut height = 1.0;
            for (li, lod) in flora::Lod::ALL.into_iter().enumerate() {
                let mesh = flora::generate_in_season(
                    &w.params,
                    u64::from(v) + 1,
                    lod,
                    flora::Season { month },
                );
                if li == 0 {
                    height = mesh.bounds().1[1].max(0.2);
                }
                let dist = lod_distances(height)[li];
                for (kind, part) in mesh.split_parts() {
                    let Some(m) = scene::upload_plant_mesh(r, &part) else {
                        continue;
                    };
                    let key = kind as usize;
                    let mat = *mats.entry(key).or_insert_with(|| {
                        let m = scene::plant_material(
                            r,
                            kind,
                            (tex.atlas, tex.atlas_normal, tex.bark, tex.bark_normal),
                            mesh.foliage_tint,
                        );
                        made.push((m, kind, mesh.foliage_tint));
                        m
                    });
                    parts.push((m, mat, dist));
                    kinds.push(kind);
                }
            }
            if id != LAWN_SPECIES && (height >= TREE_M || v == 0) {
                self.bakes.push((id.clone(), class, v));
            }
            variants.push(Variant {
                parts,
                kinds,
                baked: None,
                height,
            });
        }
        for (m, kind, tint) in made {
            self.plant_mats.push((id.clone(), m, kind, tint));
        }
        self.classes.insert((id, class), variants);
        self.rev += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn flowering_changes_the_class() {
        let mut p = flora::species()[0].params.clone();
        p.deciduous = false;
        p.bloom_months = vec![5];
        assert_ne!(class(&p, 5), class(&p, 6));
        assert_eq!(class(&p, 6), class(&p, 7));
    }
}
