//! The project as an agent reads it (`document.inspect`), and the queries on its geometry
//! (`query.raycast`, `query.ground_height`, `query.ground_length`, `query.sun_hours`), in world
//! space.

use crate::host::{HostState, missing_json};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use visioncraft_core::sun_hours::{
    FULL_SUN_HOURS, Occluder, PART_SHADE_HOURS, SunSample, day_sun, days_in_month, lit_intervals,
    plant_occluder, season_date,
};
use visioncraft_core::{
    Generator, ImportedModel, Project, Season, Session, Shape, Transform, area_lifts,
    evaluate_in_scene, evaluate_surface_fill, measure, placed_shape_mesh, plant_size, raycast,
    raycast_any, water_areas,
};

/// A model's placement: scale, then rotation about +Y, then translation.
fn to_world(t: &Transform, p: [f32; 3]) -> [f32; 3] {
    let (s, c) = t.rotation_y_deg.to_radians().sin_cos();
    let q = p.map(|v| v * t.scale);
    [
        q[0] * c + q[2] * s + t.translation[0],
        q[1] + t.translation[1],
        -q[0] * s + q[2] * c + t.translation[2],
    ]
}

fn rotate(t: &Transform, v: [f32; 3], inverse: bool) -> [f32; 3] {
    let a = t.rotation_y_deg.to_radians() * if inverse { -1.0 } else { 1.0 };
    let (s, c) = a.sin_cos();
    [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c]
}

fn to_local(t: &Transform, p: [f32; 3], is_point: bool) -> [f32; 3] {
    let d = if is_point {
        [0, 1, 2].map(|k| p[k] - t.translation[k])
    } else {
        p
    };
    rotate(t, d, true).map(|v| v / t.scale)
}

/// Something rays hit and plants stand on.
struct Owner {
    id: String,
    kind: &'static str,
    mesh: Arc<ImportedModel>,
    /// `None`: the mesh is in world space already (shapes).
    xf: Option<Transform>,
    /// Plants stand on it (areas and models, not house blocks).
    ground: bool,
}

/// The visible owners of `project` with their meshes, shapes placed as the window places them
/// (`visioncraft_core::placed_shape_mesh`).
fn owners(project: &Project, state: &HostState) -> Vec<Owner> {
    let lifts: HashMap<String, f32> = area_lifts(&project.shapes).into_iter().collect();
    let water = water_areas(project);
    let mut out = Vec::new();
    for m in &project.models {
        if let (true, Some(l)) = (project.layer_visible(&m.layer), state.models.get(&m.id)) {
            out.push(Owner {
                id: m.id.clone(),
                kind: "model",
                mesh: l.model.clone(),
                xf: Some(m.transform),
                ground: true,
            });
        }
    }
    for s in &project.shapes {
        if !project.layer_visible(s.layer()) {
            continue;
        }
        if let Ok((mesh, lift)) = placed_shape_mesh(project, s, &lifts, &water) {
            out.push(Owner {
                id: s.id().to_string(),
                kind: "shape",
                mesh: Arc::new(mesh),
                xf: (lift != 0.0).then(|| Transform {
                    translation: [0.0, lift, 0.0],
                    ..Transform::default()
                }),
                ground: matches!(s, Shape::Area(_)),
            });
        }
    }
    out
}

/// The nearest hit of a world ray: `{owner, kind, face, point, normal, distance}`.
fn cast(project: &Project, owners: &[Owner], origin: [f32; 3], dir: [f32; 3]) -> Option<Value> {
    let mut best: Option<(f32, Value)> = None;
    for o in owners {
        let (lo, ld) = match &o.xf {
            Some(t) => (to_local(t, origin, true), to_local(t, dir, false)),
            None => (origin, dir),
        };
        let Some(hit) = raycast(&o.mesh, lo, ld) else {
            continue;
        };
        if best.as_ref().is_some_and(|(d, _)| *d <= hit.distance) {
            continue;
        }
        let normal =
            o.xf.as_ref()
                .map_or(hit.normal, |t| rotate(t, hit.normal, false));
        let point = [0, 1, 2].map(|k| origin[k] + dir[k] * hit.distance);
        let face = &o.mesh.faces[hit.face];
        best = Some((
            hit.distance,
            json!({
                "owner": o.id, "kind": o.kind, "face": face.name,
                "body": o.mesh.bodies[face.body as usize].id,
                "point": point, "normal": normal, "distance": hit.distance,
            }),
        ));
    }
    if let Some(t) = &project.terrain
        && let Some((d, point, normal)) = t.raycast(origin, dir)
        && best.as_ref().is_none_or(|(b, _)| d < *b)
    {
        best = Some((
            d,
            json!({"owner": "terrain", "kind": "terrain", "face": null, "point": point, "normal": normal, "distance": d}),
        ));
    }
    best.map(|(_, v)| v)
}

/// Height of the ground plants stand on at `(x, z)`: terrain, areas and models (`models`
/// false: terrain and areas only, the open ground of the building rule).
fn ground_at(project: &Project, owners: &[Owner], x: f32, z: f32, models: bool) -> Option<f32> {
    let terrain = project.terrain.as_ref().and_then(|t| t.height_at(x, z));
    owners
        .iter()
        .filter(|o| o.ground && (models || o.kind == "shape"))
        .filter_map(|o| {
            let top = o
                .mesh
                .bodies
                .iter()
                .map(|b| b.bounds[1][1])
                .fold(f32::MIN, f32::max);
            let world_top =
                o.xf.as_ref()
                    .map_or(top, |t| t.translation[1] + top * t.scale);
            let hit = cast(
                project,
                std::slice::from_ref(o),
                [x, world_top + 1.0, z],
                [0.0, -1.0, 0.0],
            )?;
            hit["point"][1].as_f64().map(|y| y as f32)
        })
        .chain(terrain)
        .reduce(f32::max)
}

/// The placements of a generator, in world space.
fn placements(
    project: &Project,
    owners: &[Owner],
    assets: &[visioncraft_core::Asset],
    g: &Generator,
) -> (Vec<visioncraft_core::Placement>, bool) {
    match g {
        Generator::SurfaceFill(f) => match owners.iter().find(|o| o.id == f.owner) {
            Some(o) => {
                let r = evaluate_surface_fill(f, &o.mesh);
                let mut p = r.placements;
                if let Some(t) = &o.xf {
                    for x in &mut p {
                        x.position = to_world(t, x.position);
                    }
                }
                (p, r.truncated)
            }
            None => (Vec::new(), false),
        },
        _ => {
            // The window's rule (`Scene::evaluate_generator`): no plants in or under buildings.
            let up = |p: [f32; 3]| {
                owners
                    .iter()
                    .filter(|o| !(o.ground && o.kind == "shape"))
                    .filter_map(|o| {
                        // The mesh alone: `cast` would also find the terrain under the plant.
                        let (lo, ld) = match &o.xf {
                            Some(t) => (to_local(t, p, true), to_local(t, [0.0, 1.0, 0.0], false)),
                            None => (p, [0.0, 1.0, 0.0]),
                        };
                        // The local direction is the world's over the scale: distances agree.
                        raycast(&o.mesh, lo, ld).map(|h| h.distance)
                    })
                    .reduce(f32::min)
            };
            let species = |id: &str| {
                visioncraft_flora::Species::by_id(id)
                    .map(|s| (s.mature_height_m, s.mature_width_m / 2.0))
            };
            let mut e = evaluate_in_scene(
                g,
                &|x, z| ground_at(project, owners, x, z, true),
                &|x, z| ground_at(project, owners, x, z, false),
                &up,
                &|a| plant_size(project, assets, a, &species),
            );
            visioncraft_core::keep_dry(project.sea.as_ref(), &mut e, &|a| {
                assets
                    .iter()
                    .any(|x| x.id == a && visioncraft_core::water::floats(x))
            });
            (e.placements, e.truncated)
        }
    }
}

fn generator_assets(g: &Generator) -> Vec<String> {
    let v = serde_json::to_value(g).unwrap_or_default();
    let mut assets: Vec<String> = Vec::new();
    let mut add = |a: &Value| {
        if let Some(a) = a.as_str()
            && !assets.iter().any(|x| x == a)
        {
            assets.push(a.to_string());
        }
    };
    add(&v["asset"]);
    let palettes = std::iter::once(&v["asset_palette"]).chain(
        v["layers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|l| &l["asset_palette"]),
    );
    for p in palettes {
        for w in p.as_array().into_iter().flatten() {
            add(&w["asset"]);
        }
    }
    assets
}

/// The world-space box of a mesh, `None` for an empty one.
fn world_bounds(mesh: &ImportedModel, xf: Option<&Transform>) -> Option<[[f32; 3]; 2]> {
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for b in &mesh.bodies {
        for i in 0..8 {
            let p = [0, 1, 2].map(|k| b.bounds[(i >> k) & 1][k]);
            let w = xf.map_or(p, |t| to_world(t, p));
            for k in 0..3 {
                lo[k] = lo[k].min(w[k]);
                hi[k] = hi[k].max(w[k]);
            }
        }
    }
    lo[0].is_finite().then_some([lo, hi])
}

fn bbox(mesh: &ImportedModel, xf: Option<&Transform>) -> Value {
    world_bounds(mesh, xf).map_or(Value::Null, |b| json!(b))
}

/// What `document.inspect` returns.
pub(crate) struct Verbosity {
    pub faces: bool,
    pub measures: bool,
    pub placements: bool,
    pub full: bool,
}

pub(crate) fn inspect(session: &Session, state: &HostState, v: &Verbosity) -> Value {
    let p = session.project();
    let owners = owners(p, state);
    let faces_of = |id: &str| -> Value {
        owners
            .iter()
            .find(|o| o.id == id)
            .map(|o| {
                o.mesh
                    .faces
                    .iter()
                    .map(|f| f.name.clone())
                    .collect::<Vec<_>>()
                    .into()
            })
            .unwrap_or(Value::Null)
    };
    let models: Vec<Value> = p
        .models
        .iter()
        .map(|m| {
            let loaded = state.models.get(&m.id);
            let mut o = json!({
                "id": m.id, "name": m.name, "kind": m.kind, "source": m.source, "layer": m.layer,
                "transform": m.transform, "loaded": loaded.is_some(),
            });
            if let Some(l) = loaded {
                o["bodies"] = l.model.bodies.len().into();
                o["triangles"] = l.model.triangles.len().into();
                o["bbox"] = bbox(&l.model, Some(&m.transform));
                if !l.model.warnings.is_empty() {
                    o["warnings"] = json!(l.model.warnings);
                }
                if v.faces {
                    o["faces"] = l
                        .model
                        .faces
                        .iter()
                        .map(|f| f.name.clone())
                        .collect::<Vec<_>>()
                        .into();
                }
            }
            if let Some((_, why)) = state.missing.get(&m.id) {
                o["error"] = why.as_str().into();
            }
            o
        })
        .collect();
    let shapes: Vec<Value> = p
        .shapes
        .iter()
        .map(|s| {
            let mut o = serde_json::to_value(s).unwrap_or_default();
            if v.measures {
                let area = measure::polygon_area(s.outline());
                match s {
                    Shape::Area(_) => o["area_m2"] = area.into(),
                    Shape::Extrusion(e) if e.closed => o["footprint_m2"] = area.into(),
                    Shape::Extrusion(_) => {
                        let pts: Vec<[f32; 3]> =
                            s.outline().iter().map(|q| [q[0], 0.0, q[1]]).collect();
                        o["length_m"] = measure::polyline_length(&pts).into();
                    }
                }
            }
            if v.faces {
                o["faces"] = faces_of(s.id());
            }
            o
        })
        .collect();
    let generators: Vec<Value> = p
        .generators
        .iter()
        .map(|g| {
            let mut o = if v.full {
                serde_json::to_value(g).unwrap_or_default()
            } else {
                let t = serde_json::to_value(g).unwrap_or_default();
                json!({"id": g.id(), "type": t["type"], "name": g.name(), "layer": g.layer(), "assets": generator_assets(g)})
            };
            if v.measures || v.placements {
                let (list, truncated) = placements(p, &owners, &state.catalog.assets, g);
                o["count"] = list.len().into();
                o["truncated"] = truncated.into();
                if v.placements {
                    o["placements"] = list
                        .iter()
                        .map(|x| json!({"asset": x.asset, "position": x.position, "rotation_y_deg": x.rotation_y_deg, "scale": x.scale}))
                        .collect::<Vec<_>>()
                        .into();
                }
            }
            o
        })
        .collect();
    let materials: Vec<Value> = p
        .materials
        .iter()
        .map(|m| {
            if v.full {
                serde_json::to_value(m).unwrap_or_default()
            } else {
                json!({"id": m.id, "name": m.name})
            }
        })
        .collect();
    let layers: Vec<Value> = p
        .layers
        .iter()
        .map(|l| {
            let items = p.models.iter().filter(|m| m.layer == l.id).count()
                + p.shapes.iter().filter(|s| s.layer() == l.id).count()
                + p.generators.iter().filter(|g| g.layer() == l.id).count()
                + p.lights.iter().filter(|x| x.layer == l.id).count();
            json!({"id": l.id, "name": l.name, "visible": l.visible, "context": l.context, "items": items})
        })
        .collect();
    let species: Vec<Value> = p
        .species
        .iter()
        .map(|s| {
            if v.full {
                serde_json::to_value(s).unwrap_or_default()
            } else {
                json!({"id": s.id, "name": s.name, "base": s.base})
            }
        })
        .collect();
    let terrain = p.terrain.as_ref().map(|t| {
        let e = t.extent();
        json!({"extent": e, "vertices": t.vertex_count(), "cell_m": t.cell_m, "layers": t.layers.iter().map(|l| l.material.clone()).collect::<Vec<_>>()})
    });
    let mut out = Map::new();
    out.insert(
        "path".into(),
        state.file.as_ref().map(|f| f.display().to_string()).into(),
    );
    out.insert("dirty".into(), (state.saved.as_ref() != Some(p)).into());
    out.insert("revision".into(), session.revision().into());
    out.insert("site".into(), json!(p.site));
    out.insert("time".into(), json!(p.time));
    out.insert("season".into(), json!(p.season()));
    out.insert("sun".into(), sun_json(p));
    out.insert("view".into(), json!(p.view));
    out.insert("style".into(), json!(p.style));
    out.insert("layers".into(), layers.into());
    out.insert("models".into(), models.into());
    out.insert("shapes".into(), shapes.into());
    out.insert("materials".into(), materials.into());
    out.insert("assignments".into(), json!(p.assignments));
    out.insert("generators".into(), generators.into());
    out.insert("species".into(), species.into());
    out.insert("lights".into(), json!(p.lights));
    out.insert("viewpoints".into(), json!(p.viewpoints));
    out.insert("terrain".into(), terrain.into());
    // The sea with what it means: the waves' height and length from its sea state.
    out.insert(
        "sea".into(),
        p.sea.as_ref().map_or(Value::Null, |s| {
            let mut v = json!(s);
            v["wave_height_m"] = json!(s.wave_height_m());
            v["peak_wavelength_m"] = json!(s.peak_wavelength_m());
            v
        }),
    );
    let missing = missing_json(state, p);
    if !missing.is_empty() {
        out.insert("missingModels".into(), missing.into());
    }
    out.insert(
        "history".into(),
        json!({"can_undo": session.can_undo(), "can_redo": session.can_redo()}),
    );
    Value::Object(out)
}

/// The sun for the project's site and time: the core's `query.sun`.
pub(crate) fn sun_json(p: &Project) -> Value {
    Session::new(p.clone())
        .run("query.sun", json!({}))
        .unwrap_or(Value::Null)
}

pub(crate) fn raycast_json(
    session: &Session,
    state: &HostState,
    origin: [f32; 3],
    dir: [f32; 3],
) -> Value {
    let p = session.project();
    match cast(p, &owners(p, state), origin, dir) {
        Some(mut hit) => {
            hit["hit"] = true.into();
            hit
        }
        None => json!({"hit": false}),
    }
}

pub(crate) fn ground_json(session: &Session, state: &HostState, x: f32, z: f32) -> Value {
    let p = session.project();
    json!({"x": x, "z": z, "height": ground_at(p, &owners(p, state), x, z, true)})
}

/// `query.ground_length`: a walk over the ground.
pub(crate) fn ground_length_json(session: &Session, state: &HostState, path: &[[f32; 2]]) -> Value {
    let p = session.project();
    let owners = owners(p, state);
    json!({"ground_length_m": measure::polyline_length_on_ground(path, &|x, z| ground_at(p, &owners, x, z, true))})
}

/// What `query.sun_hours` was asked, checked by the command.
pub(crate) struct SunHoursQuery {
    pub point: Option<[f32; 2]>,
    pub outline: Option<Vec<[f32; 2]>>,
    pub height_m: f32,
    pub date: Option<(u32, u32)>,
    pub season: Option<Season>,
    pub step_minutes: f32,
    pub plants: bool,
    pub years: Option<f32>,
    pub cell_m: f32,
}

/// The most cells an outline is sampled at.
const MAX_CELLS: usize = 10_000;

/// Whether the ray reaches the box: the slab test.
fn ray_hits_box(origin: [f32; 3], dir: [f32; 3], b: &[[f32; 3]; 2]) -> bool {
    let (mut t0, mut t1) = (0.0f32, f32::INFINITY);
    for k in 0..3 {
        if dir[k] == 0.0 {
            if origin[k] < b[0][k] || origin[k] > b[1][k] {
                return false;
            }
            continue;
        }
        let (a, c) = (
            (b[0][k] - origin[k]) / dir[k],
            (b[1][k] - origin[k]) / dir[k],
        );
        t0 = t0.max(a.min(c));
        t1 = t1.min(a.max(c));
    }
    t0 <= t1
}

/// What casts shadow: the visible owners (each with its world box, to skip the ones a ray
/// cannot reach), the terrain and the plant crowns.
struct Shadows<'a> {
    project: &'a Project,
    owners: Vec<(&'a Owner, [[f32; 3]; 2])>,
    crowns: Vec<Occluder>,
}

impl Shadows<'_> {
    /// The share (0..=1) of the sun's light that reaches `origin` along `dir`: 0 behind a model,
    /// shape or the terrain, else the product of what the crowns in the way let through.
    fn light(&self, origin: [f32; 3], dir: [f32; 3]) -> f32 {
        for (o, b) in &self.owners {
            if !ray_hits_box(origin, dir, b) {
                continue;
            }
            let (lo, ld) = match &o.xf {
                Some(t) => (to_local(t, origin, true), to_local(t, dir, false)),
                None => (origin, dir),
            };
            if raycast_any(&o.mesh, lo, ld) {
                return 0.0;
            }
        }
        if let Some(t) = &self.project.terrain
            && t.raycast(origin, dir).is_some()
        {
            return 0.0;
        }
        self.crowns.iter().map(|c| c.pass(origin, dir)).product()
    }
}

/// The crowns of the project's woody plants `years` after planting, as ellipsoids.
fn plant_crowns(
    project: &Project,
    owners: &[Owner],
    assets: &[visioncraft_core::Asset],
    month: u32,
    years: f32,
) -> Vec<Occluder> {
    let mut out = Vec::new();
    for g in &project.generators {
        if !project.layer_visible(g.layer()) {
            continue;
        }
        for pl in placements(project, owners, assets, g).0 {
            // Species of the library say whether they keep their leaves; any other asset is
            // counted as an evergreen: opaque.
            let evergreen = pl
                .asset
                .strip_prefix("flora:")
                .and_then(visioncraft_flora::Species::by_id)
                .is_none_or(|s| s.evergreen);
            let age = g.planted_age_years().unwrap_or_else(|| {
                visioncraft_core::growth::growth_of(&pl.asset, None).planted_age_years
            }) + years;
            out.extend(plant_occluder(
                &pl.asset,
                pl.position,
                pl.scale,
                age,
                evergreen,
                month,
                project.site.latitude,
            ));
        }
    }
    out
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// `query.sun_hours`: hours of direct sun at a point or over an outline, on a date.
pub(crate) fn sun_hours_json(
    session: &Session,
    state: &HostState,
    q: &SunHoursQuery,
) -> Result<Value, String> {
    const ID: &str = "query.sun_hours";
    let p = session.project();
    if q.point.is_some() == q.outline.is_some() {
        return Err(format!("{ID}: give exactly one of `point` and `outline`"));
    }
    if !(0.0..=100.0).contains(&q.height_m) {
        return Err(format!("{ID}: `height_m` must be 0..100"));
    }
    if !(1.0..=60.0).contains(&q.step_minutes) {
        return Err(format!("{ID}: `step_minutes` must be 1..60"));
    }
    if let Some(y) = q.years
        && !(0.0..=visioncraft_core::YEARS_MAX).contains(&y)
    {
        return Err(format!(
            "{ID}: `years` must be 0..{}",
            visioncraft_core::YEARS_MAX
        ));
    }
    if q.date.is_some() && q.season.is_some() {
        return Err(format!("{ID}: give `date` or `season`, not both"));
    }
    let year = p.time.year;
    let (month, day) = match (q.date, q.season) {
        (Some((m, d)), _) => {
            if !(1..=12).contains(&m) || d < 1 || d > days_in_month(year, m) {
                return Err(format!(
                    "{ID}: `date` {{month: {m}, day: {d}}} is not a day of {year}"
                ));
            }
            (m, d)
        }
        (None, Some(s)) => season_date(s, p.site.latitude),
        (None, None) => (p.time.month, p.time.day),
    };
    // A grid first: a request that is too large is refused before any ray is cast.
    struct Grid {
        outline: Vec<[f32; 2]>,
        origin: [f32; 2],
        cols: usize,
        rows: usize,
    }
    let grid = match &q.outline {
        Some(o) => {
            if o.len() < 3 {
                return Err(format!("{ID}: `outline` needs at least 3 points"));
            }
            if !(0.01..=1000.0).contains(&q.cell_m) {
                return Err(format!("{ID}: `cell_m` must be 0.01..1000"));
            }
            let lo = [0, 1].map(|k| o.iter().map(|v| v[k]).fold(f32::INFINITY, f32::min));
            let hi = [0, 1].map(|k| o.iter().map(|v| v[k]).fold(f32::NEG_INFINITY, f32::max));
            let size = |k: usize| (((hi[k] - lo[k]) / q.cell_m).ceil() as usize).max(1);
            let (cols, rows) = (size(0), size(1));
            let too_many = || {
                format!(
                    "{ID}: the outline would need more than {MAX_CELLS} cells at `cell_m` {}; use a larger `cell_m` (about {:.1} m would do)",
                    q.cell_m,
                    q.cell_m * ((cols * rows) as f32 / MAX_CELLS as f32).sqrt().max(1.0) * 1.1
                )
            };
            if cols.saturating_mul(rows) > 25 * MAX_CELLS {
                return Err(too_many());
            }
            Some(Grid {
                outline: o.clone(),
                origin: lo,
                cols,
                rows,
            })
        }
        None => None,
    };
    let cells: Vec<Option<[f32; 2]>> = grid.as_ref().map_or_else(Vec::new, |g| {
        (0..g.rows * g.cols)
            .map(|i| {
                let c = [
                    g.origin[0] + ((i % g.cols) as f32 + 0.5) * q.cell_m,
                    g.origin[1] + ((i / g.cols) as f32 + 0.5) * q.cell_m,
                ];
                visioncraft_core::inside_outline(c, &g.outline).then_some(c)
            })
            .collect()
    });
    if cells.iter().flatten().count() > MAX_CELLS {
        let n = cells.iter().flatten().count();
        return Err(format!(
            "{ID}: the outline has {n} cells at `cell_m` {}, at most {MAX_CELLS} are sampled; use a larger `cell_m` (about {:.1} m would do)",
            q.cell_m,
            q.cell_m * (n as f32 / MAX_CELLS as f32).sqrt() * 1.1
        ));
    }

    let day_sun = day_sun(&p.site, year, month, day, f64::from(q.step_minutes));
    let owners = owners(p, state);
    let years = q.years.or(p.view.years).unwrap_or(0.0);
    let crowns = if q.plants {
        plant_crowns(p, &owners, &state.catalog.assets, month, years)
    } else {
        Vec::new()
    };
    let shadows = Shadows {
        project: p,
        owners: owners
            .iter()
            .filter_map(|o| Some((o, world_bounds(&o.mesh, o.xf.as_ref())?)))
            .collect(),
        crowns,
    };
    // The light at a spot for every slice of the day.
    let lit_at = |x: f32, z: f32| -> Vec<(SunSample, f32)> {
        let y = ground_at(p, &owners, x, z, true).unwrap_or(0.0) + q.height_m;
        // A little above the sample so the ray does not start in its own ground.
        let origin = [x, y + 0.01, z];
        day_sun
            .samples
            .iter()
            .map(|s| (*s, shadows.light(origin, s.direction)))
            .collect()
    };
    let hours_of = |lit: &[(SunSample, f32)]| -> f64 {
        lit.iter().map(|(s, l)| s.hours() * f64::from(*l)).sum()
    };
    let date = json!({"month": month, "day": day});
    let daylight = r2(day_sun.daylight_hours);
    match (q.point, grid) {
        (Some([x, z]), _) => {
            let lit = lit_at(x, z);
            let height = ground_at(p, &owners, x, z, true).unwrap_or(0.0) + q.height_m;
            Ok(json!({
                "point": [x, z], "height": r3(f64::from(height)), "date": date,
                "sunrise_hour": day_sun.sunrise.map(r2), "sunset_hour": day_sun.sunset.map(r2),
                "daylight_hours": daylight, "sun_hours": r2(hours_of(&lit)),
                "intervals": lit_intervals(&lit).iter().map(|i| [r2(i[0]), r2(i[1])]).collect::<Vec<_>>(),
                "crowns": shadows.crowns.len(), "years": years,
            }))
        }
        (None, Some(g)) => {
            let hours: Vec<Option<f64>> = cells
                .iter()
                .map(|c| c.map(|[x, z]| hours_of(&lit_at(x, z))))
                .collect();
            let inside: Vec<f64> = hours.iter().flatten().copied().collect();
            if inside.is_empty() {
                return Err(format!(
                    "{ID}: no cell centre lies inside the outline; use a smaller `cell_m`"
                ));
            }
            let n = inside.len() as f64;
            let share =
                |f: &dyn Fn(f64) -> bool| r3(inside.iter().filter(|h| f(**h)).count() as f64 / n);
            let count = |lo: f64, hi: f64| inside.iter().filter(|h| **h >= lo && **h < hi).count();
            Ok(json!({
                "date": date, "cell_m": q.cell_m, "origin": g.origin,
                "cols": g.cols, "rows": g.rows,
                "hours": hours.iter().map(|h| h.map(r2)).collect::<Vec<_>>(),
                "min": r2(inside.iter().copied().fold(f64::INFINITY, f64::min)),
                "max": r2(inside.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
                "mean": r2(inside.iter().sum::<f64>() / n),
                "daylight_hours": daylight,
                "histogram": {
                    "0-2": count(0.0, 2.0), "2-4": count(2.0, 4.0),
                    "4-6": count(4.0, 6.0), "6+": count(6.0, f64::INFINITY),
                },
                "full_sun_share": share(&|h| h >= FULL_SUN_HOURS),
                "part_shade_share": share(&|h| (PART_SHADE_HOURS..FULL_SUN_HOURS).contains(&h)),
                "shade_share": share(&|h| h < PART_SHADE_HOURS),
                "cells": inside.len(), "crowns": shadows.crowns.len(), "years": years,
            }))
        }
        (None, None) => unreachable!("one of point and outline was checked"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_and_local_are_inverse() {
        let t = Transform {
            translation: [3.0, 1.0, -2.0],
            rotation_y_deg: 90.0,
            scale: 2.0,
        };
        // +X turned 90 degrees counter-clockwise from above is -Z.
        let w = to_world(&t, [1.0, 0.0, 0.0]);
        assert!(
            (w[0] - 3.0).abs() < 1e-5 && (w[2] + 4.0).abs() < 1e-5,
            "{w:?}"
        );
        let back = to_local(&t, w, true);
        assert!(
            (back[0] - 1.0).abs() < 1e-5 && back[2].abs() < 1e-5,
            "{back:?}"
        );
    }
}
