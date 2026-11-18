//! The project's terrain on the GPU: one LOD item per 64-cell chunk (three mesh levels), remade
//! only where the heights or paint changed, and the painted layers from the layers' materials.

use crate::library::{Library, TexSrc};
use crate::scene::{Look, MaterialId, MeshId};
use glam::Mat4;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use visioncraft_core::Project;
use visioncraft_core::terrain::{
    CHUNK_CELLS, DirtyRect, Terrain, terrain_chunk_mesh_holes, terrain_chunks,
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
    /// The ground plane height the chunks were made for (borders at it get no skirt).
    ground: Option<u32>,
    chunks: HashMap<(u32, u32), Chunk>,
    layers: Vec<TerrainLayer>,
    /// The footprints of terraces cut out of the chunks (`visioncraft_core::terrace`).
    holes: Vec<Vec<[f32; 2]>>,
}

/// The chunks of `t` the polygons `polys` reach.
fn chunks_under(t: &Terrain, polys: &[&Vec<[f32; 2]>]) -> BTreeSet<(u32, u32)> {
    let (cx, cz) = terrain_chunks(t);
    let span = |lo: f32, hi: f32, origin: f32, n: u32| {
        let at =
            |v: f32| (((v - origin) / t.cell_m).floor() as i64).div_euclid(i64::from(CHUNK_CELLS));
        let last = i64::from(n) - 1;
        at(lo - t.cell_m).clamp(0, last)..=at(hi + t.cell_m).clamp(0, last)
    };
    let mut out = BTreeSet::new();
    for poly in polys {
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in poly.iter() {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        if cx == 0 || cz == 0 || lo[0] > hi[0] {
            continue;
        }
        for z in span(lo[1], hi[1], t.origin[1], cz) {
            for x in span(lo[0], hi[0], t.origin[0], cx) {
                out.insert((x as u32, z as u32));
            }
        }
    }
    out
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
        ground: Option<f32>,
        holes: &[Vec<[f32; 2]>],
        material: MaterialId,
    ) -> Option<DirtyRect> {
        if self.ground != ground.map(f32::to_bits) {
            self.clear(r);
            self.ground = ground.map(f32::to_bits);
        }
        // Which footprints changed: the chunks they touch (before and after) are made again.
        let old_holes = std::mem::replace(&mut self.holes, holes.to_vec());
        let moved: Vec<&Vec<[f32; 2]>> = old_holes
            .iter()
            .filter(|h| !holes.contains(h))
            .chain(holes.iter().filter(|h| !old_holes.contains(h)))
            .collect();
        let Some(t) = terrain else {
            let had = self.shown.as_ref().map(|t| t.whole());
            self.clear(r);
            return had;
        };
        if self.shown.as_ref().is_some_and(|s| Arc::ptr_eq(s, t)) {
            for (x, z) in chunks_under(t, &moved) {
                if self.chunks.contains_key(&(x, z)) {
                    self.remake(r, t, x, z, ground, material);
                }
            }
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
        let Some(d) = dirty.filter(|d| !d.is_empty()) else {
            for (x, z) in chunks_under(t, &moved) {
                self.remake(r, t, x, z, ground, material);
            }
            return None;
        };
        // Chunks whose vertices (shared edges included) the rectangle touches.
        let (cx, cz) = terrain_chunks(t);
        let span = |lo: u32, hi: u32, n: u32| {
            let a = lo.saturating_sub(1) / CHUNK_CELLS;
            let b = (hi / CHUNK_CELLS).min(n.saturating_sub(1));
            a..=b
        };
        let mut made = BTreeSet::new();
        for z in span(d.z0, d.z1, cz) {
            for x in span(d.x0, d.x1, cx) {
                self.remake(r, t, x, z, ground, material);
                made.insert((x, z));
            }
        }
        for (x, z) in chunks_under(t, &moved) {
            if !made.contains(&(x, z)) {
                self.remake(r, t, x, z, ground, material);
            }
        }
        Some(d)
    }

    fn remake(
        &mut self,
        r: &mut Renderer,
        t: &Terrain,
        x: u32,
        z: u32,
        ground: Option<f32>,
        material: MaterialId,
    ) {
        let mut meshes = Vec::new();
        for (step, _) in LEVELS {
            let m = terrain_chunk_mesh_holes(t, x, z, step, ground, &self.holes);
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
                let (base, normal, orm, height) = lib.layer_textures(look.tex as usize);
                TerrainLayer {
                    base_color_map: base,
                    normal_map: normal,
                    orm_map: orm,
                    height_map: height,
                    scale_m: look.scale_m.max(0.1),
                    base_color: look.color,
                    roughness: look.roughness,
                    variation: look.variation,
                    relief: look.relief,
                    roughness_offset: -look.gloss,
                    puddles: look.puddles,
                }
            })
            .collect();
        if layers != self.layers && r.set_terrain_layers(&layers).is_ok() {
            self.layers = layers;
        }
    }
}

impl Library {
    /// The colour, normal, packed and height maps of interned spec `index` once loaded (asks
    /// for them).
    pub fn layer_textures(
        &mut self,
        index: usize,
    ) -> (
        Option<visioncraft_render::TextureId>,
        Option<visioncraft_render::TextureId>,
        Option<visioncraft_render::TextureId>,
        Option<visioncraft_render::TextureId>,
    ) {
        if index == 0 {
            return (None, None, None, None);
        }
        let Some(spec) = self.specs.get(index).cloned() else {
            return (None, None, None, None);
        };
        let mut get = |src: &Option<TexSrc>, srgb: bool| match src {
            Some(TexSrc::Lib(p)) => self.store.texture(p, srgb, true, 5.0),
            _ => None,
        };
        (
            get(&spec.base, true),
            get(&spec.normal, false),
            get(&spec.orm, false),
            get(&spec.height, false),
        )
    }
}
