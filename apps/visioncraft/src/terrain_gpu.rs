//! The project's terrain on the GPU: one LOD item per 64-cell chunk (three mesh levels), remade
//! only where the heights or paint changed, and the painted layers from the layers' materials.

use crate::library::{Library, TexSrc};
use crate::scene::{Look, MaterialId, MeshId};
use glam::Mat4;
use std::collections::HashMap;
use std::sync::Arc;
use visioncraft_core::Project;
use visioncraft_core::terrain::{
    CHUNK_CELLS, DirtyRect, Terrain, terrain_chunk_mesh, terrain_chunks,
};
use visioncraft_render::{ItemId, Renderer, TerrainChunk, TerrainLayer};

/// Vertex steps of the three levels, and how far each is drawn (metres).
const LEVELS: [(u32, f32); 3] = [(1, 70.0), (2, 180.0), (4, f32::INFINITY)];

struct Chunk {
    meshes: Vec<MeshId>,
    item: ItemId,
}

#[derive(Default)]
pub struct TerrainGpu {
    shown: Option<Arc<Terrain>>,
    chunks: HashMap<(u32, u32), Chunk>,
    layers: Vec<TerrainLayer>,
}

/// The grid of `t`, to know whether a new terrain fits the chunks there are.
fn grid(t: &Terrain) -> ([u32; 2], u32, u32, u32) {
    (
        [t.origin[0].to_bits(), t.origin[1].to_bits()],
        t.cell_m.to_bits(),
        t.width,
        t.depth,
    )
}

impl TerrainGpu {
    pub fn clear(&mut self, r: &mut Renderer) {
        for (_, c) in self.chunks.drain() {
            r.remove_item(c.item);
            c.meshes.into_iter().for_each(|m| r.remove_mesh(m));
        }
        self.shown = None;
    }

    /// Brings the chunks in line with `terrain`; returns the vertex rectangle that changed (the
    /// whole grid for a new or resized terrain), `None` when nothing did.
    pub fn sync(
        &mut self,
        r: &mut Renderer,
        terrain: Option<&Arc<Terrain>>,
        material: MaterialId,
    ) -> Option<DirtyRect> {
        let Some(t) = terrain else {
            let had = self.shown.as_ref().map(|t| t.whole());
            self.clear(r);
            return had;
        };
        if self.shown.as_ref().is_some_and(|s| Arc::ptr_eq(s, t)) {
            return None;
        }
        let dirty = match &self.shown {
            Some(old) if grid(old) == grid(t) => t.changed_since(old),
            _ => {
                self.clear(r);
                Some(t.whole())
            }
        };
        self.shown = Some(t.clone());
        let d = dirty?;
        if d.is_empty() {
            return None;
        }
        // Chunks whose vertices (shared edges included) the rectangle touches.
        let (cx, cz) = terrain_chunks(t);
        let span = |lo: u32, hi: u32, n: u32| {
            let a = lo.saturating_sub(1) / CHUNK_CELLS;
            let b = (hi / CHUNK_CELLS).min(n.saturating_sub(1));
            a..=b
        };
        for z in span(d.z0, d.z1, cz) {
            for x in span(d.x0, d.x1, cx) {
                self.remake(r, t, x, z, material);
            }
        }
        Some(d)
    }

    fn remake(&mut self, r: &mut Renderer, t: &Terrain, x: u32, z: u32, material: MaterialId) {
        let mut meshes = Vec::new();
        for (step, _) in LEVELS {
            let m = terrain_chunk_mesh(t, x, z, step);
            let chunk = TerrainChunk {
                positions: &m.positions,
                normals: &m.normals,
                indices: &m.indices,
                weights: &m.weights,
            };
            let id = match self.chunks.get(&(x, z)).map(|c| c.meshes[meshes.len()]) {
                Some(id) if r.replace_terrain_chunk(id, &chunk).is_ok() => Some(id),
                _ => r.add_terrain_chunk(&chunk).ok(),
            };
            meshes.extend(id);
        }
        if let Some(old) = self.chunks.get(&(x, z))
            && old.meshes == meshes
        {
            return; // replaced in place: the item stays
        }
        if let Some(old) = self.chunks.remove(&(x, z)) {
            r.remove_item(old.item);
            old.meshes
                .into_iter()
                .filter(|m| !meshes.contains(m))
                .for_each(|m| r.remove_mesh(m));
        }
        if meshes.len() != LEVELS.len() {
            meshes.into_iter().for_each(|m| r.remove_mesh(m));
            return;
        }
        let parts: Vec<_> = meshes
            .iter()
            .zip(LEVELS)
            .map(|(&m, (_, d))| (m, material, d))
            .collect();
        let item = r.add_lod_item(&parts, &[Mat4::IDENTITY]);
        self.chunks.insert((x, z), Chunk { meshes, item });
    }

    /// The layers from their materials' looks (pictures from the library once loaded); set on
    /// the renderer only when they change.
    pub fn sync_layers(
        &mut self,
        r: &mut Renderer,
        project: &Project,
        lib: &mut Library,
        look: &mut dyn FnMut(&visioncraft_core::Material, &mut Library) -> Look,
    ) {
        let Some(t) = &project.terrain else {
            return;
        };
        let layers: Vec<TerrainLayer> = t
            .layers
            .iter()
            .map(|l| {
                let Some(m) = project.materials.iter().find(|m| m.id == l.material) else {
                    return TerrainLayer::default();
                };
                let look = look(m, lib);
                let (base, normal, orm) = lib.layer_textures(look.tex as usize);
                TerrainLayer {
                    base_color_map: base,
                    normal_map: normal,
                    orm_map: orm,
                    scale_m: look.scale_m.max(0.1),
                    base_color: look.color,
                    roughness: look.roughness,
                    variation: look.variation,
                }
            })
            .collect();
        if layers != self.layers && r.set_terrain_layers(&layers).is_ok() {
            self.layers = layers;
        }
    }
}

impl Library {
    /// The colour, normal and packed maps of interned spec `index` once loaded (asks for them).
    pub fn layer_textures(
        &mut self,
        index: usize,
    ) -> (
        Option<visioncraft_render::TextureId>,
        Option<visioncraft_render::TextureId>,
        Option<visioncraft_render::TextureId>,
    ) {
        if index == 0 {
            return (None, None, None);
        }
        let Some(spec) = self.specs.get(index).cloned() else {
            return (None, None, None);
        };
        let mut get = |src: &Option<TexSrc>, srgb: bool| match src {
            Some(TexSrc::Lib(p)) => self.store.texture(p, srgb, true, 5.0),
            _ => None,
        };
        (
            get(&spec.base, true),
            get(&spec.normal, false),
            get(&spec.orm, false),
        )
    }
}
