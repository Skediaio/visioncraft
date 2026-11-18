//! Natural ground on the GPU (`visioncraft_core::natural`): rocks and deadwood as instanced
//! items, one per generated variant (or per catalogue scan once it is resident), and the
//! terrain's ground cover round the eye, made again only when the eye moves to another cell.

use super::{Look, MaterialId, MeshId, Scene, clutter, look_of};
use glam::{Mat4, Quat, Vec3};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use visioncraft_core::natural::{
    CoverKind, DeadwoodKind, RockShape, cover_spots, deadwood_matrix, deadwood_mesh,
    model_matrix_at, model_scale, rock_matrix, rock_mesh,
};
use visioncraft_core::{Project, Shape, inside_outline};
use visioncraft_render::{ItemId, Renderer};

/// Generated rocks and deadwood draw their fine mesh within this distance, the coarse one beyond.
const FINE_M: f32 = 45.0;
/// The ground cover is drawn to here; the last `COVER_FADE_BAND_M` dither out.
const COVER_M: f32 = 25.0;
const COVER_FADE_BAND_M: f32 = 10.0;
/// The cover is made round the eye's cell of this size, out to `COVER_M` beyond its edge.
pub(super) const COVER_CELL_M: f32 = 8.0;
/// Snow this deep (`view.snow`) covers the ground cover.
const SNOW_BURIES: f32 = 0.3;
/// The library is told where the scans stand under this name.
const NOTE: &str = "~natural";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Key {
    Rock(RockShape, u32),
    Wood(DeadwoodKind, u32, u32),
}

/// An outline with its bounding box `[min, max]`.
type Outline = (Vec<[f32; 2]>, [[f32; 2]; 2]);

/// One generated mesh's levels: (main fine, main coarse, second fine, second coarse).
type Levels = (MeshId, MeshId, Option<(MeshId, MeshId)>);

#[derive(Default)]
pub(super) struct Natural {
    key: Option<u64>,
    items: Vec<ItemId>,
    meshes: HashMap<Key, Levels>,
    cover_key: Option<u64>,
    cover_items: Vec<ItemId>,
    /// The stones (3 fine), the twig, the stones (3 coarse), then the moss cushion.
    cover_meshes: Vec<(MeshId, MaterialId)>,
    moss_mesh: Option<MeshId>,
    /// The eye's cell (`COVER_CELL_M` squares), and whether the cover follows it.
    pub(super) eye: [i32; 2],
    pub(super) follow: bool,
}

impl Natural {
    pub(super) fn clear(&mut self, r: &mut Renderer) {
        self.items.drain(..).for_each(|i| r.remove_item(i));
        self.cover_items.drain(..).for_each(|i| r.remove_item(i));
        self.key = None;
        self.cover_key = None;
    }
}

fn upload(r: &mut Renderer, p: &[[f32; 3]], n: &[[f32; 3]], idx: &[u32]) -> Option<MeshId> {
    if idx.is_empty() {
        return None;
    }
    r.add_mesh(&visioncraft_render::Mesh {
        positions: p,
        normals: n,
        indices: idx,
        face_ids: None,
        uvs: None,
        wind: None,
    })
    .ok()
}

fn levels(r: &mut Renderer, key: Key) -> Option<Levels> {
    let make = |fine: bool| match key {
        Key::Rock(shape, v) => rock_mesh(shape, v, fine),
        Key::Wood(kind, v, bucket) => deadwood_mesh(kind, v, bucket, fine),
    };
    let (f, c) = (make(true), make(false));
    let main = (
        upload(r, &f.positions, &f.normals, &f.indices[..f.split])?,
        upload(r, &c.positions, &c.normals, &c.indices[..c.split])?,
    );
    let second = match (
        upload(r, &f.positions, &f.normals, &f.indices[f.split..]),
        upload(r, &c.positions, &c.normals, &c.indices[c.split..]),
    ) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    };
    Some((main.0, main.1, second))
}

/// A moss cushion of radius 1: a low dome whose rim dips under the ground, its edge frayed.
fn moss_cushion() -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<u32>) {
    const SIDES: u32 = 14;
    const RINGS: u32 = 4;
    let mut p = vec![[0.0, 0.09, 0.0]];
    for i in 1..=RINGS {
        let t = i as f32 / RINGS as f32;
        for k in 0..SIDES {
            let a = k as f32 / SIDES as f32 * std::f32::consts::TAU;
            // Lobes on the rim, so no two cushions read as discs.
            let lobe = 1.0 + 0.18 * (a * 3.0).sin() * t + 0.1 * (a * 5.0 + 1.3).cos() * t;
            let y = 0.09 * (1.0 - t * t) - 0.06 * t * t * t;
            p.push([a.cos() * t * lobe, y, a.sin() * t * lobe]);
        }
    }
    let mut idx = Vec::new();
    for k in 0..SIDES {
        idx.extend([0, 1 + (k + 1) % SIDES, 1 + k]);
    }
    for i in 0..RINGS - 1 {
        for k in 0..SIDES {
            let a = 1 + i * SIDES + k;
            let b = 1 + i * SIDES + (k + 1) % SIDES;
            idx.extend([a, b + SIDES, a + SIDES, a, b, b + SIDES]);
        }
    }
    let n = clutter::normals(&p, &idx);
    (p, n, idx)
}

impl Scene {
    fn natural_material(
        &mut self,
        r: &mut Renderer,
        project: &Project,
        id: &str,
        fallback: [f32; 3],
    ) -> MaterialId {
        let month = u32::from(project.plant_month());
        match project.materials.iter().find(|m| m.id == id) {
            Some(m) => {
                let look = Look {
                    stripes: 0,
                    aging: [0.0; 4],
                    ..look_of(m, &mut self.library, month, &project.site, 0.0)
                };
                self.material(r, &look)
            }
            None => {
                let look = Look::plain(fallback[0], fallback[1], fallback[2], 0.9);
                self.material(r, &look)
            }
        }
    }

    pub(super) fn sync_natural(&mut self, r: &mut Renderer, project: &Project) {
        self.sync_rocks_and_wood(r, project);
        self.sync_cover(r, project);
    }

    fn sync_rocks_and_wood(&mut self, r: &mut Renderer, project: &Project) {
        let rocks: Vec<_> = project
            .rocks
            .iter()
            .filter(|x| !x.hidden && project.layer_visible(&x.layer))
            .collect();
        let wood: Vec<_> = project
            .deadwood
            .iter()
            .filter(|x| !x.hidden && project.layer_visible(&x.layer))
            .collect();
        // What it is drawn from: the items, the ground under them and the scans resident.
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{rocks:?}{wood:?}").hash(&mut h);
        self.terrain
            .as_ref()
            .map(|t| std::sync::Arc::as_ptr(t) as usize)
            .hash(&mut h);
        format!("{:?}", self.zones.len()).hash(&mut h);
        for o in self.owners.values().filter(|o| o.is_area || !o.is_shape) {
            o.xf.to_cols_array().map(f32::to_bits).hash(&mut h);
            format!("{:?}", o.key.geom).hash(&mut h);
        }
        let models: Vec<&str> = rocks
            .iter()
            .filter_map(|x| x.model.as_deref())
            .chain(wood.iter().filter_map(|x| x.model.as_deref()))
            .collect();
        for m in &models {
            (m, self.library.parts(m).is_some()).hash(&mut h);
        }
        for id in rocks
            .iter()
            .map(|x| &x.material)
            .chain(wood.iter().map(|x| &x.material))
        {
            format!("{:?}", project.materials.iter().find(|m| &m.id == id)).hash(&mut h);
        }
        project.plant_month().hash(&mut h);
        let key = h.finish();
        if self.natural.key == Some(key) {
            return;
        }
        self.natural.items.drain(..).for_each(|i| r.remove_item(i));
        self.natural.key = Some(key);
        self.library.forget_generator(NOTE);
        if rocks.is_empty() && wood.is_empty() {
            return;
        }

        // Generated variants: (key, material) -> matrices; scans: model -> matrices.
        let mut generated: HashMap<(Key, String), Vec<Mat4>> = HashMap::new();
        let mut scans: HashMap<String, Vec<Mat4>> = HashMap::new();
        {
            let ground = self.ground_fn();
            let size_of = |m: &str| {
                self.library
                    .catalog
                    .as_ref()
                    .and_then(|c| c.asset(m))
                    .map(|a| a.size_m)
            };
            for x in &rocks {
                if let Some(m) = &x.model
                    && let Some(size) = size_of(m)
                {
                    let s = model_scale(size, x.size_m);
                    let xf = model_matrix_at(
                        x.position,
                        x.yaw_deg,
                        x.align,
                        size,
                        s,
                        x.sink * 0.5,
                        &ground,
                    );
                    scans
                        .entry(m.clone())
                        .or_default()
                        .push(Mat4::from_cols_array(&xf));
                    if self.library.parts(m).is_some() {
                        continue;
                    }
                }
                let k = Key::Rock(
                    x.shape,
                    (x.seed % u64::from(visioncraft_core::natural::VARIANTS)) as u32,
                );
                generated
                    .entry((k, x.material.clone()))
                    .or_default()
                    .push(Mat4::from_cols_array(&rock_matrix(x, &ground)));
            }
            for x in &wood {
                if let Some(m) = &x.model
                    && let Some(size) = size_of(m)
                {
                    let s = model_scale(size, x.length_m);
                    let xf = model_matrix_at(x.position, x.yaw_deg, 0.8, size, s, 0.05, &ground);
                    scans
                        .entry(m.clone())
                        .or_default()
                        .push(Mat4::from_cols_array(&xf));
                    if self.library.parts(m).is_some() {
                        continue;
                    }
                }
                let (kind, v, bucket) = x.mesh_key();
                generated
                    .entry((Key::Wood(kind, v, bucket), x.material.clone()))
                    .or_default()
                    .push(Mat4::from_cols_array(&deadwood_matrix(x, &ground)));
            }
        }
        let mut items = Vec::new();
        let mut keys: Vec<_> = generated.keys().cloned().collect();
        keys.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        let timber = self.natural_material(r, project, "timber", [0.34, 0.2, 0.09]);
        for k in keys {
            let list = &generated[&k];
            let (key, mat_id) = &k;
            if !self.natural.meshes.contains_key(key)
                && let Some(l) = levels(r, *key)
            {
                self.natural.meshes.insert(*key, l);
            }
            let Some(&(fine, coarse, second)) = self.natural.meshes.get(key) else {
                continue;
            };
            let fallback = match key {
                Key::Rock(..) => [0.3, 0.29, 0.27],
                Key::Wood(..) => [0.16, 0.12, 0.09],
            };
            let mat = self.natural_material(r, project, mat_id, fallback);
            let mut parts = vec![(fine, mat, FINE_M), (coarse, mat, f32::INFINITY)];
            if let Some((a, b)) = second {
                parts.extend([(a, timber, FINE_M), (b, timber, f32::INFINITY)]);
            }
            items.push(r.add_lod_item(&parts, list));
        }
        // The scans: drawn once resident; the library is told where they stand so it loads them.
        let mut models: Vec<_> = scans.keys().cloned().collect();
        models.sort();
        for m in models {
            let list = &scans[&m];
            self.library.note(NOTE, &m, list);
            if let Some(parts) = self.library.parts(&m) {
                items.push(r.add_lod_item(&parts, list));
            }
        }
        self.natural.items = items;
    }

    fn sync_cover(&mut self, r: &mut Renderer, project: &Project) {
        let cover = project
            .terrain
            .as_ref()
            .and_then(|t| t.cover.clone())
            .filter(|_| project.view.clutter && !self.clutter_low && project.snow() < SNOW_BURIES);
        self.natural.follow = cover.is_some();
        let Some(cover) = cover else {
            if self.natural.cover_key.is_some() {
                self.natural
                    .cover_items
                    .drain(..)
                    .for_each(|i| r.remove_item(i));
                self.natural.cover_key = None;
            }
            return;
        };
        let Some(t) = self.terrain.clone() else {
            return;
        };
        let eye = self.natural.eye;
        // Areas drawn over the terrain (paving, a lawn, a pond) and terraces hide it there.
        let areas: Vec<Outline> = project
            .shapes
            .iter()
            .filter_map(|s| match s {
                Shape::Area(a) if project.layer_visible(&a.layer) => {
                    let o = a.outline().into_owned();
                    let mut lo = [f32::MAX; 2];
                    let mut hi = [f32::MIN; 2];
                    for p in &o {
                        for k in 0..2 {
                            lo[k] = lo[k].min(p[k]);
                            hi[k] = hi[k].max(p[k]);
                        }
                    }
                    Some((o, [lo, hi]))
                }
                _ => None,
            })
            .collect();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (eye, std::sync::Arc::as_ptr(&t) as usize).hash(&mut h);
        format!("{cover:?}{areas:?}{}", self.zones.len()).hash(&mut h);
        format!("{:?}", project.materials.iter().find(|m| m.id == "moss")).hash(&mut h);
        let key = h.finish();
        if self.natural.cover_key == Some(key) {
            return;
        }
        self.natural
            .cover_items
            .drain(..)
            .for_each(|i| r.remove_item(i));
        self.natural.cover_key = Some(key);

        let centre = eye.map(|k| (k as f32 + 0.5) * COVER_CELL_M);
        let reach = COVER_M + COVER_CELL_M;
        let spots = cover_spots(&t, &cover, centre, reach);
        let covered = |x: f32, z: f32| {
            areas.iter().any(|(o, [lo, hi])| {
                x >= lo[0] && x <= hi[0] && z >= lo[1] && z <= hi[1] && inside_outline([x, z], o)
            }) || self
                .zones
                .iter()
                .any(|zone| inside_outline([x, z], &zone.hole))
        };
        let mut stones: Vec<Vec<Mat4>> = vec![Vec::new(); 3];
        let mut twigs = Vec::new();
        let mut moss = Vec::new();
        for s in &spots {
            if covered(s.x, s.z) {
                continue;
            }
            let Some(y) = t.height_at(s.x, s.z) else {
                continue;
            };
            let p = Vec3::new(s.x, y, s.z);
            match s.kind {
                CoverKind::Stone => {
                    let v = ((s.pick * 3.0) as usize).min(2);
                    stones[v].push(Mat4::from_scale_rotation_translation(
                        Vec3::splat(s.size * 0.5),
                        Quat::from_rotation_y(s.yaw),
                        p,
                    ));
                }
                CoverKind::Twig => twigs.push(Mat4::from_scale_rotation_translation(
                    Vec3::splat(s.size),
                    Quat::from_rotation_y(s.yaw),
                    p,
                )),
                CoverKind::Moss => {
                    // Square to the slope, so the rim stays in the ground.
                    let n = Vec3::from(t.normal_at(s.x, s.z));
                    let tilt = Quat::from_rotation_arc(Vec3::Y, n);
                    let r = s.size * 0.5;
                    moss.push(Mat4::from_scale_rotation_translation(
                        Vec3::new(r, r, r),
                        tilt * Quat::from_rotation_y(s.yaw),
                        p,
                    ));
                }
            }
        }
        if self.natural.cover_meshes.is_empty() {
            self.natural.cover_meshes = clutter::make_meshes(r);
        }
        if self.natural.moss_mesh.is_none() {
            let (p, n, i) = moss_cushion();
            self.natural.moss_mesh = upload(r, &p, &n, &i);
        }
        let moss_mat = self.natural_material(r, project, "moss", [0.08, 0.14, 0.03]);
        let mut items = Vec::new();
        let mut add = |r: &mut Renderer, parts: &[(MeshId, MaterialId, f32)], list: &[Mat4]| {
            if list.is_empty() {
                return;
            }
            let parts: Vec<_> = parts
                .iter()
                .map(|&(m, mat, d)| (m, mat, d.min(COVER_M)))
                .collect();
            let item = r.add_lod_item(&parts, list);
            r.set_item_cull_fade(item, COVER_FADE_BAND_M);
            items.push(item);
        };
        let meshes = self.natural.cover_meshes.clone();
        for (k, list) in stones.iter().enumerate() {
            if let (Some(&(m, mat)), Some(&(coarse, _))) = (meshes.get(k), meshes.get(4 + k)) {
                add(r, &[(m, mat, 6.0), (coarse, mat, f32::INFINITY)], list);
            }
        }
        if let Some(&(m, mat)) = meshes.get(3) {
            add(r, &[(m, mat, f32::INFINITY)], &twigs);
        }
        if let Some(m) = self.natural.moss_mesh {
            add(r, &[(m, moss_mat, f32::INFINITY)], &moss);
        }
        self.natural.cover_items = items;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moss_cushion_faces_up_and_dips_at_its_rim() {
        let (p, n, i) = moss_cushion();
        assert!(i.iter().all(|k| (*k as usize) < p.len()));
        assert!(n[0][1] > 0.9, "{:?}", n[0]);
        let rim = p.last().unwrap();
        assert!(rim[1] < 0.0);
    }
}
