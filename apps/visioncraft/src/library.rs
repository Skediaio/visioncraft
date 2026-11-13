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
use visioncraft_render::{
    AlphaMode, ColorSpace, Material, MaterialId, Mesh, MeshId, Projection, Renderer, Surface,
    TextureDesc, TextureId,
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
    /// Set when the catalogue has just arrived (the studio adds its assets for placeholders).
    pub catalog_arrived: bool,
    indexes_asked: bool,
    models: HashMap<String, Model>,
    awaiting: HashSet<String>,
    uploading: Vec<Upload>,
    garbage: Vec<Model>,
    /// Where each generator's library assets stand, to see how near the camera they are.
    wants: HashMap<String, Vec<(String, Vec<Vec3>)>>,
    pub lamps_on: bool,
    /// Large-scale variation per project material (not stored in the project yet).
    pub variation: HashMap<String, f32>,
    lamps_applied: Option<bool>,
    /// Bumps whenever something a scene item is built from changed.
    pub rev: u64,
    specs: Vec<TexSpec>,
    textured: Vec<Textured>,
    mem: HashMap<String, Rgba>,
    mem_tex: HashMap<String, TextureId>,
    resolved_store_rev: u64,
    dirty: bool,
    pub messages: Vec<String>,
    pub worst_pump_ms: f64,
}

impl Default for Library {
    fn default() -> Self {
        Self::new()
    }
}

fn free(r: &mut Renderer, m: Model) {
    // A material keeps its textures alive through its bind group until it is set again.
    for id in &m.materials {
        r.set_material(*id, &Material::default());
    }
    m.meshes.into_iter().for_each(|id| r.remove_mesh(id));
    m.textures.into_iter().for_each(|id| r.remove_texture(id));
}

impl Library {
    pub fn new() -> Self {
        Self {
            store: AssetStore::new(),
            catalog: None,
            materials: Vec::new(),
            skies: Vec::new(),
            catalog_arrived: false,
            indexes_asked: false,
            models: HashMap::new(),
            awaiting: HashSet::new(),
            uploading: Vec::new(),
            garbage: Vec::new(),
            wants: HashMap::new(),
            lamps_on: false,
            variation: HashMap::new(),
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

    /// The resident parts of a catalogue asset as (mesh, material, max distance) of an LOD item:
    /// each level uses its own model, else the next coarser one in memory, else a finer one.
    /// `None` while nothing of it is resident (the caller keeps the placeholder).
    pub fn parts(&self, asset: &str) -> Option<Vec<(MeshId, MaterialId, f32)>> {
        let a = self.catalog.as_ref()?.asset(asset)?;
        let n = a.lods.len();
        let loaded: Vec<Option<&Model>> = a.lods.iter().map(|l| self.models.get(&l.file)).collect();
        let mut out = Vec::new();
        for (i, lod) in a.lods.iter().enumerate() {
            let m = (i..n)
                .find_map(|j| loaded[j])
                .or_else(|| (0..i).rev().find_map(|j| loaded[j]))?;
            out.extend(
                m.parts
                    .iter()
                    .map(|&(mesh, mat)| (mesh, mat, lod.max_distance_m)),
            );
        }
        Some(out)
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

    /// Decoded pixels (a dropped `.mcmat` map) usable as `TexSrc::Mem(name)`.
    pub fn add_memory_image(&mut self, name: String, img: Rgba) {
        self.mem.insert(name, img);
        self.dirty = true;
    }

    /// Marks the pictures of the project's materials as used (and asks again for any that were
    /// evicted), so memory pressure takes the models' finer levels before them.
    fn touch_textured(&mut self) {
        for i in 0..self.textured.len() {
            let spec = self.specs[self.textured[i].spec].clone();
            for (src, srgb) in [
                (&spec.base, true),
                (&spec.normal, false),
                (&spec.orm, false),
                (&spec.roughness, false),
                (&spec.metallic, false),
                (&spec.ao, false),
            ] {
                if let Some(TexSrc::Lib(p)) = src {
                    let _ = self.store.texture(p, srgb, true, 10.0);
                }
            }
        }
    }

    fn resolve_textured(&mut self, r: &mut Renderer) {
        for i in 0..self.textured.len() {
            let spec = self.specs[self.textured[i].spec].clone();
            let mut m = self.textured[i].base;
            let mut pending = false;
            let mut get =
                |this: &mut Self, src: &Option<TexSrc>, srgb: bool| -> Option<TextureId> {
                    match src {
                        None => None,
                        Some(TexSrc::Lib(p)) => {
                            let id = this.store.texture(p, srgb, true, 10.0);
                            pending |= id.is_none();
                            id
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

    // ---- the frame ----

    fn read_indexes(&mut self) {
        let catalog = Key::Bytes("catalog.json".into());
        if let Some(Loaded::Bytes(b)) = self.store.take(&catalog) {
            match Catalog::from_json_bytes(&b) {
                Ok(c) => {
                    self.catalog = Some(Arc::new(c));
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
        let Some(cat) = self.catalog.clone() else {
            return;
        };
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
            for (i, l) in a.lods.iter().enumerate() {
                // Over the memory cap only the coarsest levels are asked for; the finer ones
                // that are resident then stop being used and are the first to go.
                if i == n - 1 || (i == need && !self.store.over_cap) {
                    self.request(&l.file, priority(d, i, n - 1));
                }
            }
        }
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
        for p in &data.glb.prims {
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
                positions: &p.positions,
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
        self.rev != rev_before
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
        let json = format!(
            "{{\"texture_bytes\":{},\"peak_texture_bytes\":{},\"uploads\":{},\"worst_upload_ms\":{:.2},\"total_upload_ms\":{:.1},\"evictions\":{},\"queued\":{},\"models\":{},\"worst_pump_ms\":{:.2}}}",
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
        !self.store.busy() && self.uploading.is_empty() && self.awaiting.is_empty()
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
    let packed = matches!((&maps.roughness, &maps.metallic, &maps.ambient_occlusion),
        (Some(a), Some(b), Some(c)) if a == b && b == c);
    TexSpec {
        base: src(&maps.base_color),
        normal: src(&maps.normal),
        orm: if packed { src(&maps.roughness) } else { None },
        roughness: if packed { None } else { src(&maps.roughness) },
        metallic: if packed { None } else { src(&maps.metallic) },
        ao: if packed {
            None
        } else {
            src(&maps.ambient_occlusion)
        },
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
