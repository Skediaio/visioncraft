//! Real plants from `visioncraft-flora`: generated lazily (textures of one species, or the meshes
//! of one species and season class, per frame), uploaded once, drawn as LOD items.

use crate::scene::{self, MaterialId, MeshId};
use std::collections::HashMap;
use visioncraft_core::Project;
use visioncraft_flora as flora;
use visioncraft_render::{Renderer, TextureId};

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

struct Tex {
    atlas: TextureId,
    bark: TextureId,
    bark_normal: TextureId,
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
    /// (species, class) to parts per variant: (mesh, material, LOD max distance).
    classes: HashMap<(String, u32), Vec<Vec<Part>>>,
    queue: Vec<Want>,
    /// Counts finished generations, so generators know to rebuild.
    pub rev: u64,
}

impl Flora {
    pub fn ready(&self, id: &str, class: u32) -> bool {
        self.classes.contains_key(&(id.to_string(), class))
    }

    pub fn parts(&self, id: &str, class: u32, variant: u32) -> Option<&[Part]> {
        let v = self.classes.get(&(id.to_string(), class))?;
        v.get((variant % VARIANTS) as usize).map(Vec::as_slice)
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

    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// One step: the textures of the first wanted species, or its meshes. True when something finished.
    pub fn pump(&mut self, r: &mut Renderer) -> bool {
        let Some(w) = self.queue.first() else {
            return false;
        };
        let (id, class, month) = (w.id.clone(), w.class, w.month);
        if !self.textures.contains_key(&id) {
            let t = flora::textures(&w.params);
            let up = |r: &mut Renderer, i: &flora::Image, srgb: bool, repeat: bool| {
                scene::upload_texture(r, i.width, i.height, &i.rgba, srgb, repeat)
            };
            let tex = (|| {
                Some(Tex {
                    atlas: up(r, &t.atlas, true, false)?,
                    bark: up(r, &t.bark, true, true)?,
                    bark_normal: up(r, &t.bark_normal, false, true)?,
                })
            })();
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
        let mut variants = Vec::new();
        for v in 0..VARIANTS {
            let mut parts = Vec::new();
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
                let dist = [8.0 + 3.0 * height, 22.0 + 6.0 * height, f32::INFINITY][li];
                for (kind, part) in mesh.split_parts() {
                    let Some(m) = scene::upload_plant_mesh(r, &part) else {
                        continue;
                    };
                    let key = kind as usize;
                    let mat = *mats.entry(key).or_insert_with(|| {
                        scene::plant_material(
                            r,
                            kind,
                            (tex.atlas, tex.bark, tex.bark_normal),
                            mesh.foliage_tint,
                        )
                    });
                    parts.push((m, mat, dist));
                }
            }
            variants.push(parts);
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
