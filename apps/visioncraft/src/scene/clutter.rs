//! Ground clutter: fallen leaves under deciduous trees (season aware, made from the tree's own
//! leaf shapes and scans), small stones and twigs on soil and gravel, and grass blades hanging
//! over a lawn's edge. Cheap instanced cards and small meshes that dither out by `FADE_M`; left
//! out at LOW quality and when the project's `view.clutter` is false.

use super::{LAWN_SPECIES, MaterialId, MeshId, Scene, tree_crowns};
use crate::plants::{self, CLUTTER_PREFIX};
use glam::{Mat4, Vec3};
use std::hash::{Hash, Hasher};
use visioncraft_core::material::Lawn3d;
use visioncraft_core::{
    Material, Project, Shape, SurfaceKind, areas_over, ground_height, inside_outline, lawn_scatter,
    resolve_materials,
};
use visioncraft_flora as flora;
use visioncraft_render::{ItemId, Renderer};

/// Clutter is drawn to here; the last `FADE_BAND_M` dither out.
const FADE_M: f32 = 25.0;
const FADE_BAND_M: f32 = 10.0;
/// One leaf patch (a flat ground cover of the tree's leaves) is this wide, unscaled.
const PATCH_M: f32 = 0.35;
/// Fallen-leaf patches per square metre of crown, by month: most just after leaf fall, rotting
/// away through winter and spring, a few early drops in summer.
const LEAVES_BY_MONTH: [f32; 12] = [1.8, 1.4, 1.0, 0.6, 0.3, 0.2, 0.2, 0.4, 1.0, 4.0, 4.8, 2.8];
/// Twigs per square metre under a crown (all year) and on soil.
const TWIGS_UNDER_CROWN: f32 = 0.6;
const TWIGS_ON_SOIL: f32 = 0.5;
/// Stones per square metre on soil and on gravel (bigger ones on top of the gravel texture).
const STONES_ON_SOIL: f32 = 6.0;
const STONES_ON_GRAVEL: f32 = 20.0;
/// Overhanging grass tufts along a lawn's edge, this far apart.
const EDGE_SPACING_M: f32 = 0.03;
/// At most this many instances of one kind.
const MAX_INSTANCES: usize = 40_000;
const STONE_VARIANTS: usize = 3;

/// What is drawn and what it was made from.
#[derive(Default)]
pub(super) struct Clutter {
    key: Option<u64>,
    items: Vec<ItemId>,
    /// Some species it needs are still being generated.
    waiting: bool,
    /// The stone variants, then the twig: (mesh, material), made once.
    meshes: Vec<(MeshId, MaterialId)>,
}

impl Clutter {
    pub(super) fn clear(&mut self, r: &mut Renderer) {
        self.items.drain(..).for_each(|i| r.remove_item(i));
        self.key = None;
        self.waiting = false;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Ground {
    /// A lawn, with its blades' colour.
    Lawn([f32; 3]),
    Soil,
    Gravel,
}

fn ground_of(m: &Material, blades: impl Fn() -> [f32; 3]) -> Option<Ground> {
    if matches!(m.kind, SurfaceKind::Lawn3d(_)) {
        return Some(Ground::Lawn(blades()));
    }
    let name = format!("{} {}", m.id, m.name).to_lowercase();
    let any = |w: &[&str]| w.iter().any(|w| name.contains(w));
    if any(&["soil", "mulch", "earth", "bed"]) {
        Some(Ground::Soil)
    } else if any(&["gravel", "grit", "shingle", "pebble"]) {
        Some(Ground::Gravel)
    } else {
        None
    }
}

impl Scene {
    /// Clutter waiting for a species still being generated (pictures wait for it).
    pub fn clutter_pending(&self) -> usize {
        usize::from(self.clutter.waiting && self.flora.pending() > 0)
    }

    /// LOW quality leaves clutter out. True when it changed (the next sync drops or makes it).
    pub fn set_clutter_low(&mut self, low: bool) -> bool {
        std::mem::replace(&mut self.clutter_low, low) != low
    }

    pub(super) fn sync_clutter(&mut self, r: &mut Renderer, project: &Project) {
        let month = u32::from(project.plant_month());
        // Fallen leaves, stones and edge grass lie under the snow.
        let on = project.view.clutter && !self.clutter_low && project.view.snow < SNOW_BURIES;
        // Deciduous trees drop leaves round their crowns: those placed one by one, and those a
        // planting area, line, brush or cluster scatters (their placements, as drawn).
        let deciduous = |asset: &str| {
            let g = visioncraft_core::growth::growth_of(asset, None);
            matches!(g.grower, visioncraft_core::growth::Grower::Tree)
                .then(|| plants::resolve(project, asset))
                .flatten()
                .filter(|r| r.params.deciduous)
                .map(|r| (g, r))
        };
        let mut trees: Vec<(String, flora::PlantParams, Option<String>, [f32; 3])> =
            tree_crowns(project)
                .into_iter()
                .filter(|(s, _)| on && project.layer_visible(&s.layer))
                .filter_map(|(s, c)| {
                    let res = plants::resolve(project, &s.asset)?;
                    res.params.deciduous.then(|| {
                        (
                            format!("{CLUTTER_PREFIX}leaves:{}", s.asset),
                            litter(&res.params),
                            res.scan_id,
                            c,
                        )
                    })
                })
                .collect();
        for g in project.generators.iter().filter(|g| {
            on && !matches!(g, visioncraft_core::Generator::Single(_))
                && project.layer_visible(g.layer())
                && g.assets().into_iter().any(|a| deciduous(a).is_some())
        }) {
            for p in self.evaluate_generator(g).placements {
                let Some((gr, res)) = deciduous(&p.asset) else {
                    continue;
                };
                let width = match project.view.years {
                    Some(y) => {
                        let planted = g.planted_age_years().unwrap_or(gr.planted_age_years);
                        gr.mature_width_m * gr.size(planted + y).width
                    }
                    None => gr.mature_width_m,
                };
                trees.push((
                    format!("{CLUTTER_PREFIX}leaves:{}", p.asset),
                    litter(&res.params),
                    res.scan_id,
                    [p.position[0], p.position[2], width * p.scale * 0.5],
                ));
            }
        }
        let mut owners: Vec<&String> = self.owners.keys().collect();
        owners.sort();
        let mut grounds: Vec<(String, Vec<(usize, Ground)>)> = Vec::new();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (
            on,
            month,
            self.terrain
                .as_ref()
                .map(|t| std::sync::Arc::as_ptr(t) as usize),
        )
            .hash(&mut h);
        for id in owners {
            let o = &self.owners[id];
            (
                id,
                format!("{:?}", o.key.geom),
                o.key.visible,
                o.xf.to_cols_array().map(f32::to_bits),
            )
                .hash(&mut h);
            if !on || !o.key.visible || !o.is_shape {
                continue;
            }
            let faces: Vec<(usize, Ground)> = resolve_materials(project, id, &o.geom)
                .into_iter()
                .enumerate()
                .filter_map(|(f, m)| {
                    let m = project.materials.get(m?)?;
                    Some((f, ground_of(m, || self.blade_color(m, month))?))
                })
                .collect();
            if !faces.is_empty() {
                format!("{faces:?}{:?}", areas_over(&project.shapes, id)).hash(&mut h);
                grounds.push((id.clone(), faces));
            }
        }
        let lawn = plants::resolve(project, LAWN_SPECIES)
            .filter(|_| {
                grounds
                    .iter()
                    .any(|g| g.1.iter().any(|f| matches!(f.1, Ground::Lawn(_))))
            })
            .map(|res| (plants::class(&res.params, month as u8), res.params));
        for (id, p, _, c) in &trees {
            (
                id,
                c.map(f32::to_bits),
                self.flora.ready(id, plants::class(p, month as u8)),
            )
                .hash(&mut h);
        }
        if let Some((class, _)) = &lawn {
            self.flora.ready(LAWN_SPECIES, *class).hash(&mut h);
        }
        let key = h.finish();
        if self.clutter.key == Some(key) {
            return;
        }
        self.clutter.clear(r);
        self.clutter.key = Some(key);
        if !on {
            return;
        }

        // The species it needs: wanted when missing, drawn once they are there.
        let mut waiting = false;
        if let Some((class, params)) = &lawn
            && self.flora.parts(LAWN_SPECIES, *class, 0).is_none()
        {
            self.flora
                .want(LAWN_SPECIES, None, *class, month as u8, params);
            waiting = true;
        }
        for (id, p, scan, _) in &trees {
            let class = plants::class(p, month as u8);
            if self.flora.parts(id, class, 0).is_none() {
                self.flora.want(id, scan.as_deref(), class, month as u8, p);
                waiting = true;
            }
        }
        self.clutter.waiting = waiting;

        let variants = plants::VARIANTS as usize;
        let mut leaves: Vec<Vec<Vec<Mat4>>> = Vec::new();
        let mut stones: Vec<Vec<Mat4>> = vec![Vec::new(); STONE_VARIANTS];
        let mut twigs: Vec<Mat4> = Vec::new();
        let mut edges: Vec<Vec<Mat4>> = vec![Vec::new(); variants];
        let mut edge_color = None;
        {
            let ground = self.ground_fn();
            for (i, (_, _, _, c)) in trees.iter().enumerate() {
                let mut by_v = vec![Vec::new(); variants];
                for (k, s) in leaf_spots(*c, month, i as u64).into_iter().enumerate() {
                    if let Some(y) = ground(s[0], s[1]) {
                        by_v[k % variants].push(placed(Vec3::new(s[0], y, s[1]), s[2], s[3]));
                    }
                }
                leaves.push(by_v);
                for s in disc_spots(*c, 1.0, TWIGS_UNDER_CROWN, 1000 + i as u64) {
                    if let Some(y) = ground(s[0], s[1]) {
                        twigs.push(placed(Vec3::new(s[0], y, s[1]), s[2], twig_length(s[3])));
                    }
                }
            }
        }
        for (id, faces) in &grounds {
            let o = &self.owners[id];
            let over = areas_over(&project.shapes, id);
            let free = |p: Vec3| !over.iter().any(|a| inside_outline([p.x, p.z], a));
            for &(face, g) in faces {
                let (stone_density, twig_density) = match g {
                    Ground::Lawn(c) => {
                        edge_color.get_or_insert(c);
                        continue;
                    }
                    Ground::Soil => (STONES_ON_SOIL, TWIGS_ON_SOIL),
                    Ground::Gravel => (STONES_ON_GRAVEL, 0.0),
                };
                let scatter = |density: f32, seed: u64| {
                    lawn_scatter(
                        &o.geom,
                        face,
                        &Lawn3d {
                            density_per_m2: density,
                            blade_height_m: 0.04,
                            seed,
                        },
                    )
                    .into_iter()
                    .map(|t| {
                        (
                            o.xf.transform_point3(Vec3::from(t.position)),
                            t.rotation_y_deg,
                        )
                    })
                    .filter(|(p, _)| free(*p))
                    .collect::<Vec<_>>()
                };
                for (k, (p, yaw)) in scatter(stone_density, 17).into_iter().enumerate() {
                    let u = unit(hash(k as u64 ^ p.x.to_bits() as u64));
                    // Mostly small, a few larger ones.
                    let size = 0.012 + 0.04 * u * u * u;
                    stones[k % STONE_VARIANTS].push(placed(p, yaw.to_radians(), size));
                }
                for (k, (p, yaw)) in scatter(twig_density, 29).into_iter().enumerate() {
                    let u = unit(hash(k as u64 ^ p.z.to_bits() as u64));
                    twigs.push(placed(p, yaw.to_radians(), twig_length(u)));
                }
            }
            // Grass over the lawn's outline and over the areas drawn on top of it (a path,
            // a terrace, a pond).
            if !faces.iter().any(|f| matches!(f.1, Ground::Lawn(_))) {
                continue;
            }
            let Some(Shape::Area(a)) = project.shapes.iter().find(|s| s.id() == id.as_str()) else {
                continue;
            };
            let mut lines = polygon_edges(&a.outline, false);
            for o in &over {
                lines.extend(polygon_edges(o, true));
            }
            let inv = o.xf.inverse();
            // Grass hangs over paving, a border or water: another drawn area that is not a
            // lawn. Where lawn meets lawn or the bare ground round it, there is no edge to see.
            let others: Vec<(&[[f32; 2]], bool)> = project
                .shapes
                .iter()
                .filter_map(|s| match s {
                    Shape::Area(b) if b.id != *id && project.layer_visible(&b.layer) => {
                        let lawn = self.owners.get(&b.id).is_some_and(|ob| {
                            resolve_materials(project, &b.id, &ob.geom)
                                .into_iter()
                                .flatten()
                                .filter_map(|m| project.materials.get(m))
                                .any(|m| matches!(m.kind, SurfaceKind::Lawn3d(_)))
                        });
                        Some((b.outline.as_slice(), lawn))
                    }
                    _ => None,
                })
                .collect();
            for (k, s) in edge_spots(&lines, EDGE_SPACING_M, 3)
                .into_iter()
                .enumerate()
            {
                if !inside_outline(s.at, &a.outline) || over.iter().any(|o| inside_outline(s.at, o))
                {
                    continue;
                }
                let beyond = [s.at[0] + s.out[0] * 0.1, s.at[1] + s.out[1] * 0.1];
                if !others
                    .iter()
                    .rev()
                    .find(|(outline, _)| inside_outline(beyond, outline))
                    .is_some_and(|(_, lawn)| !lawn)
                {
                    continue;
                }
                let local = inv.transform_point3(Vec3::new(s.at[0], 0.0, s.at[1]));
                let Some(y) = ground_height(&o.geom, local.x, local.z) else {
                    continue;
                };
                let p = o.xf.transform_point3(Vec3::new(local.x, y, local.z));
                edges[k % variants].push(overhang(p, s.out, s.yaw, s.size, s.tilt));
            }
        }

        // Into the renderer.
        let mut items = Vec::new();
        let mut add = |r: &mut Renderer, parts: &[(MeshId, MaterialId, f32)], list: &[Mat4]| {
            if list.is_empty() {
                return;
            }
            let parts: Vec<_> = parts
                .iter()
                .map(|&(m, mat, d)| (m, mat, d.min(FADE_M)))
                .collect();
            let list = &list[..list.len().min(MAX_INSTANCES)];
            let item = r.add_lod_item(&parts, list);
            r.set_item_cull_fade(item, FADE_BAND_M);
            items.push(item);
        };
        for ((id, p, _, _), by_v) in trees.iter().zip(&leaves) {
            let class = plants::class(p, month as u8);
            let tint = litter_tint(p, month);
            for (v, list) in by_v.iter().enumerate() {
                if let Some(parts) = self.flora.tinted(r, id, class, v as u32, tint) {
                    add(r, &parts, list);
                }
            }
        }
        if let (Some((class, _)), Some(color)) = (&lawn, edge_color) {
            for (v, list) in edges.iter().enumerate() {
                if let Some(parts) = self.flora.tinted(r, LAWN_SPECIES, *class, v as u32, color) {
                    add(r, &parts, list);
                }
            }
        }
        if self.clutter.meshes.is_empty() {
            self.clutter.meshes = make_meshes(r);
        }
        for (k, list) in stones.iter().enumerate() {
            if let Some(&(m, mat)) = self.clutter.meshes.get(k) {
                add(r, &[(m, mat, f32::INFINITY)], list);
            }
        }
        if let Some(&(m, mat)) = self.clutter.meshes.get(STONE_VARIANTS) {
            add(r, &[(m, mat, f32::INFINITY)], &twigs);
        }
        self.clutter.items = items;
    }
}

/// A fallen-leaf patch of a tree species: a flat ground cover of its leaves, a few to a card.
fn litter(tree: &flora::PlantParams) -> flora::PlantParams {
    let mut p = tree.clone();
    p.height_m = 0.03;
    p.width_m = PATCH_M;
    p.age = 1.0;
    // Not deciduous itself: every card stays, whatever the month.
    p.deciduous = false;
    p.marcescent = false;
    p.bloom_months.clear();
    p.flowers = None;
    p.budget = flora::BudgetClass::Tuft;
    p.form = flora::Form::GroundCover(flora::GroundCoverParams {
        cards: 12,
        depth_m: 0.012,
        tilt_deg: 14.0,
    });
    p.leaves.colour = p.leaves.autumn_colour;
    p.leaves.colour_var = p.leaves.colour_var.max(0.5);
    p.leaves.card_m = p.leaves.leaf_m * 1.4;
    p
}

/// The linear colour fallen leaves average: the tree's autumn colour, dulled, in autumn; dry
/// brown the rest of the year.
fn litter_tint(p: &flora::PlantParams, month: u32) -> [f32; 3] {
    let lin = |c: f32| c.clamp(0.0, 1.0).powf(2.2);
    let autumn = p.leaves.autumn_colour.map(|c| lin(c) * 0.5);
    let brown = [0.10, 0.062, 0.032];
    let t = if (9..=11).contains(&month) { 0.45 } else { 0.1 };
    [0, 1, 2].map(|k| autumn[k] * t + brown[k] * (1.0 - t))
}

fn placed(p: Vec3, yaw: f32, scale: f32) -> Mat4 {
    Mat4::from_translation(p) * Mat4::from_rotation_y(yaw) * Mat4::from_scale(Vec3::splat(scale))
}

fn twig_length(u: f32) -> f32 {
    0.08 + 0.3 * u * u
}

/// A lawn tuft at `p` leaning over towards `out` (x, z) by `tilt` radians.
fn overhang(p: Vec3, out: [f32; 2], yaw: f32, size: f32, tilt: f32) -> Mat4 {
    let axis = Vec3::Y
        .cross(Vec3::new(out[0], 0.0, out[1]))
        .normalize_or_zero();
    Mat4::from_translation(p)
        * Mat4::from_axis_angle(axis, tilt)
        * Mat4::from_rotation_y(yaw)
        * Mat4::from_scale(Vec3::splat(size))
}

fn hash(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// 0..1 from a hash.
fn unit(h: u64) -> f32 {
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Points spread over the crown circle `c` [x, z, radius] (a little past the drip line, where
/// the wind takes them) at `density` per square metre: [x, z, yaw, 0..1].
fn disc_spots(c: [f32; 3], reach: f32, density: f32, seed: u64) -> Vec<[f32; 4]> {
    let r = c[2] * reach;
    let n = ((density * std::f32::consts::PI * r * r).round() as usize).min(MAX_INSTANCES / 8);
    let mut s = hash(seed ^ (u64::from(c[0].to_bits()) << 1) ^ (u64::from(c[1].to_bits()) << 33));
    let mut next = || {
        s = hash(s);
        unit(s)
    };
    (0..n)
        .map(|_| {
            let (rad, ang) = (r * next().sqrt(), next() * std::f32::consts::TAU);
            [
                c[0] + rad * ang.cos(),
                c[1] + rad * ang.sin(),
                next() * std::f32::consts::TAU,
                next(),
            ]
        })
        .collect()
}

/// Fallen-leaf patches under crown `c` in `month`: [x, z, yaw, scale].
fn leaf_spots(c: [f32; 3], month: u32, seed: u64) -> Vec<[f32; 4]> {
    let density = LEAVES_BY_MONTH[(month.clamp(1, 12) - 1) as usize];
    disc_spots(c, 1.15, density, seed)
        .into_iter()
        .map(|s| [s[0], s[1], s[2], 0.7 + 0.6 * s[3]])
        .collect()
}

/// The edges of `outline` with their unit normal (x, z): pointing out of the polygon, or into
/// it when `inward`.
fn polygon_edges(outline: &[[f32; 2]], inward: bool) -> Vec<([f32; 2], [f32; 2], [f32; 2])> {
    let n = outline.len();
    if n < 3 {
        return Vec::new();
    }
    (0..n)
        .filter_map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
            let len = (dx * dx + dz * dz).sqrt();
            if len < 1e-4 {
                return None;
            }
            let mut out = [-dz / len, dx / len];
            let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            let probe = [mid[0] + out[0] * 0.01, mid[1] + out[1] * 0.01];
            if inside_outline(probe, outline) != inward {
                out = [-out[0], -out[1]];
            }
            Some((a, b, out))
        })
        .collect()
}

struct EdgeSpot {
    at: [f32; 2],
    out: [f32; 2],
    yaw: f32,
    size: f32,
    tilt: f32,
}

/// Tufts along `edges` every `spacing` metres, set back 0.5 to 3.5 cm from the edge (against
/// `out`) and leaning over it; uncut edge grass is a little longer than the lawn.
fn edge_spots(edges: &[([f32; 2], [f32; 2], [f32; 2])], spacing: f32, seed: u64) -> Vec<EdgeSpot> {
    let mut s = hash(seed);
    let mut next = || {
        s = hash(s);
        unit(s)
    };
    let mut spots = Vec::new();
    for &(a, b, out) in edges {
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let n = (len / spacing).ceil() as usize;
        for i in 0..n {
            if spots.len() >= MAX_INSTANCES {
                return spots;
            }
            let t = (i as f32 + next()) / n as f32;
            let back = 0.005 + 0.03 * next();
            spots.push(EdgeSpot {
                at: [
                    a[0] + (b[0] - a[0]) * t - out[0] * back,
                    a[1] + (b[1] - a[1]) * t - out[1] * back,
                ],
                out,
                yaw: next() * std::f32::consts::TAU,
                size: 0.6 + 0.4 * next(),
                tilt: 0.5 + 0.5 * next(),
            });
        }
    }
    spots
}

/// The stone variants and the twig, with their materials.
fn make_meshes(r: &mut Renderer) -> Vec<(MeshId, MaterialId)> {
    let colors = [[0.16, 0.15, 0.13], [0.11, 0.1, 0.09], [0.2, 0.17, 0.14]];
    let mut out = Vec::new();
    let mut upload =
        |r: &mut Renderer, (p, i): (Vec<[f32; 3]>, Vec<u32>), color: [f32; 3], rough: f32| {
            let n = normals(&p, &i);
            let mesh = visioncraft_render::Mesh {
                positions: &p,
                normals: &n,
                indices: &i,
                face_ids: None,
                uvs: None,
                wind: None,
            };
            if let Ok(m) = r.add_mesh(&mesh) {
                let mat = r.add_material(&visioncraft_render::Material {
                    base_color: [color[0], color[1], color[2], 1.0],
                    roughness: rough,
                    ..Default::default()
                });
                out.push((m, mat));
            }
        };
    for (k, c) in colors.into_iter().enumerate() {
        upload(r, stone(k as u64), c, 0.85);
    }
    upload(r, twig(), [0.15, 0.1, 0.065], 0.9);
    out
}

/// A pebble of radius about 1, flattened and sunk a little into the ground: a once-subdivided
/// icosahedron pushed in and out by a few waves.
fn stone(seed: u64) -> (Vec<[f32; 3]>, Vec<u32>) {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut p: Vec<Vec3> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .map(|v| Vec3::from(v).normalize())
    .to_vec();
    let ico: [[u32; 3]; 20] = [
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    let mut mid = std::collections::HashMap::new();
    let mut tris = Vec::new();
    for [a, b, c] in ico {
        let mut m = |i: u32, j: u32| {
            *mid.entry((i.min(j), i.max(j))).or_insert_with(|| {
                p.push(((p[i as usize] + p[j as usize]) / 2.0).normalize());
                p.len() as u32 - 1
            })
        };
        let (ab, bc, ca) = (m(a, b), m(b, c), m(c, a));
        tris.extend([a, ab, ca, b, bc, ab, c, ca, bc, ab, bc, ca]);
    }
    let mut s = hash(seed + 1);
    let mut next = || {
        s = hash(s);
        unit(s) * 2.0 - 1.0
    };
    let waves: Vec<(Vec3, f32)> = (0..3)
        .map(|_| {
            (
                Vec3::new(next(), next(), next()).normalize_or_zero() * 2.5,
                next() * 3.0,
            )
        })
        .collect();
    let squash = Vec3::new(1.0 + 0.2 * next(), 0.5 + 0.1 * next(), 0.8 + 0.15 * next());
    let pos = p
        .iter()
        .map(|&d| {
            let bump: f32 = waves
                .iter()
                .map(|(w, ph)| (d.dot(*w) + ph).sin())
                .sum::<f32>();
            let v = d * (1.0 + 0.07 * bump) * squash;
            [v.x, v.y - 0.2, v.z]
        })
        .collect();
    (pos, tris)
}

/// A twig of length 1 along x lying on the ground, slightly bent, five-sided.
fn twig() -> (Vec<[f32; 3]>, Vec<u32>) {
    const SIDES: u32 = 5;
    const RINGS: u32 = 4;
    let mut pos = Vec::new();
    for i in 0..RINGS {
        let t = i as f32 / (RINGS - 1) as f32;
        let r = 0.014 - 0.007 * t;
        let centre = Vec3::new(t - 0.5, r, 0.05 * (t * std::f32::consts::PI).sin());
        for k in 0..SIDES {
            let a = k as f32 / SIDES as f32 * std::f32::consts::TAU;
            pos.push((centre + Vec3::new(0.0, a.cos(), a.sin()) * r).to_array());
        }
    }
    let mut idx = Vec::new();
    for i in 0..RINGS - 1 {
        for k in 0..SIDES {
            let (a, b) = (i * SIDES + k, i * SIDES + (k + 1) % SIDES);
            let (c, d) = (a + SIDES, b + SIDES);
            idx.extend([a, c, b, b, c, d]);
        }
    }
    (pos, idx)
}

/// Smooth vertex normals, area weighted.
fn normals(p: &[[f32; 3]], idx: &[u32]) -> Vec<[f32; 3]> {
    let mut n = vec![Vec3::ZERO; p.len()];
    for t in idx.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| Vec3::from(p[i as usize]));
        let f = (b - a).cross(c - a);
        for &i in t {
            n[i as usize] += f;
        }
    }
    n.into_iter()
        .map(|v| v.try_normalize().unwrap_or(Vec3::Y).to_array())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_leaves_lie_in_autumn_and_all_under_the_crown() {
        let c = [3.0, -2.0, 2.5];
        let (nov, jun) = (leaf_spots(c, 11, 1), leaf_spots(c, 6, 1));
        assert!(
            nov.len() > 10 * jun.len().max(1),
            "{} vs {}",
            nov.len(),
            jun.len()
        );
        assert!(!jun.is_empty());
        for s in &nov {
            assert!(((s[0] - c[0]).powi(2) + (s[1] - c[1]).powi(2)).sqrt() <= c[2] * 1.15 + 1e-4);
        }
        assert_eq!(nov.len(), leaf_spots(c, 11, 1).len(), "deterministic");
    }

    #[test]
    fn edge_tufts_stand_on_the_lawn_and_lean_out_of_it() {
        // Either winding.
        for square in [
            vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]],
            vec![[0.0, 0.0], [0.0, 3.0], [4.0, 3.0], [4.0, 0.0]],
        ] {
            let edges = polygon_edges(&square, false);
            assert_eq!(edges.len(), 4);
            let spots = edge_spots(&edges, 0.05, 1);
            assert!(spots.len() >= 280, "{}", spots.len());
            for s in &spots {
                assert!(inside_outline(s.at, &square), "{:?}", s.at);
                // Leaning outward: away from the centre.
                let to = [s.at[0] - 2.0, s.at[1] - 1.5];
                assert!(to[0] * s.out[0] + to[1] * s.out[1] > 0.0);
                // The tuft's top goes outward.
                let up = overhang(Vec3::ZERO, s.out, s.yaw, 1.0, s.tilt).transform_vector3(Vec3::Y);
                assert!(up.x * s.out[0] + up.z * s.out[1] > 0.4 && up.y > 0.3);
            }
            // Into a path drawn over the lawn: the tufts stay outside it.
            let into = polygon_edges(&square, true);
            for s in edge_spots(&into, 0.05, 1) {
                assert!(!inside_outline(s.at, &square), "{:?}", s.at);
            }
        }
    }

    #[test]
    fn stone_and_twig_meshes_are_closed_enough_to_draw() {
        let (p, i) = stone(2);
        assert_eq!((p.len(), i.len()), (42, 240));
        assert!(i.iter().all(|&k| (k as usize) < p.len()));
        let (p, i) = twig();
        assert!(i.iter().all(|&k| (k as usize) < p.len()));
        assert!(
            normals(&p, &i)
                .iter()
                .all(|n| (Vec3::from(*n).length() - 1.0).abs() < 1e-3)
        );
    }

    #[test]
    fn soil_and_gravel_are_told_apart_by_name() {
        let palette = visioncraft_core::material::builtin_palette();
        let named = |id: &str| palette.iter().find(|m| m.id == id).unwrap().clone();
        let blades = || [0.0; 3];
        assert_eq!(ground_of(&named("soil"), blades), Some(Ground::Soil));
        assert_eq!(ground_of(&named("gravel"), blades), Some(Ground::Gravel));
        assert_eq!(ground_of(&named("paving"), blades), None);
        assert!(matches!(
            ground_of(&named("lawn"), blades),
            Some(Ground::Lawn(_))
        ));
    }
}
