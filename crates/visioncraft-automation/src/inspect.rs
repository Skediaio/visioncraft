//! The project as an agent reads it (`document.inspect`), and the queries on its geometry
//! (`query.raycast`, `query.ground_height`, `query.ground_length`, `query.sun_hours`), in world
//! space.

use crate::host::{HostState, missing_json};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use visioncraft_core::sun_hours::{
    FULL_SUN_HOURS, Occluder, PART_SHADE_HOURS, SunSample, day_sun, days_in_month, lit_intervals,
    plant_occluder, season_date,
};
use visioncraft_core::wall_photo::face_frame;
use visioncraft_core::{
    Generator, ImportedModel, Project, Season, Session, Shape, Target, Transform, area_lifts,
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

/// The geometry of a model by id: the host's loaded models.
type Models<'a> = dyn Fn(&str) -> Option<Arc<ImportedModel>> + 'a;

/// The models a host state holds.
fn models_of(state: &HostState) -> impl Fn(&str) -> Option<Arc<ImportedModel>> + '_ {
    |id| state.models.get(id).map(|l| l.model.clone())
}

/// The visible owners of `project` with their meshes, shapes placed as the window places them
/// (`visioncraft_core::placed_shape_mesh`).
fn owners(project: &Project, models: &Models) -> Vec<Owner> {
    let lifts: HashMap<String, f32> = area_lifts(&project.shapes).into_iter().collect();
    let water = water_areas(project);
    let mut out = Vec::new();
    for m in &project.models {
        if let (true, Some(model)) = (project.layer_visible(&m.layer), models(&m.id)) {
            out.push(Owner {
                id: m.id.clone(),
                kind: "model",
                mesh: model,
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
    // The terrain as it is drawn: a terrace's footprint is cut out of it.
    if let Some((d, point, normal)) =
        visioncraft_core::terrace::project_raycast(project, origin, dir)
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
    let terrain = visioncraft_core::terrace::ground_fn(project)(x, z);
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

/// `query.quantities` as the host answers it: the core's numbers plus the linked models, and
/// plants counted as the renderer draws them (`placements`, the building rule).
pub(crate) fn quantities_json(
    session: &Session,
    state: &HostState,
    ids: Option<&[String]>,
    layer: Option<&str>,
) -> Result<Value, String> {
    let p = session.project();
    let owners = owners(p, &models_of(state));
    let models = models_of(state);
    let scene = visioncraft_core::quantities::Scene {
        models: &models,
        placements: &|g| placements(p, &owners, &state.catalog.assets, g),
    };
    visioncraft_core::quantities::quantities_in(
        p,
        &visioncraft_core::quantities::Filter { ids, layer },
        Some(&scene),
    )
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
    let owners = owners(p, &models_of(state));
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
                let area = measure::polygon_area(&s.outline());
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
                + p.lights.iter().filter(|x| x.layer == l.id).count()
                + p.underlays.iter().filter(|u| u.layer == l.id).count();
            json!({"id": l.id, "name": l.name, "visible": l.visible, "context": l.context, "items": items})
        })
        .collect();
    let underlays: Vec<Value> = p
        .underlays
        .iter()
        .map(|u| {
            let mut o = json!({
                "id": u.id, "name": u.name, "file": u.file, "layer": u.layer,
                "origin": u.placement.origin, "metres_per_pixel": u.placement.metres_per_pixel,
                "rotation_deg": u.placement.rotation_deg, "size_px": u.size_px,
                "opacity": u.opacity, "elevation": u.elevation,
                "corners": u.corners(), "bounds": u.bounds(),
                "loaded": state.images.contains_key(&u.file),
            });
            if let Some(why) = state.image_missing.get(&u.file) {
                o["error"] = why.as_str().into();
            }
            o
        })
        .collect();
    let wall_photos: Vec<Value> = {
        p.wall_photos
            .iter()
            .map(|w| {
                let mut o = json!({
                    "id": w.id, "name": w.name, "file": w.file, "size_px": w.size_px,
                    "target": w.target, "corners_px": w.corners_px, "region": w.region,
                    "loaded": state.images.contains_key(&w.file),
                });
                if let Some(why) = state.image_missing.get(&w.file) {
                    o["error"] = why.as_str().into();
                }
                // The face's own frame, so a region can be typed against its size.
                let face = match &w.target {
                    Target::Face { owner, face } => owners
                        .iter()
                        .find(|x| x.id == *owner)
                        .ok_or_else(|| format!("no visible owner `{owner}`"))
                        .and_then(|x| {
                            let at = x
                                .mesh
                                .faces
                                .iter()
                                .position(|f| f.name == *face)
                                .ok_or_else(|| format!("`{owner}` has no face `{face}`"))?;
                            let up = match &x.xf {
                                Some(t) => to_local(t, [0.0, 1.0, 0.0], false),
                                None => [0.0, 1.0, 0.0],
                            };
                            face_frame(&x.mesh, at, up)
                        }),
                    _ => Err("the target is not a face".to_string()),
                };
                match face {
                    Ok(f) => o["face_size_m"] = json!(f.size),
                    Err(why) => o["face_error"] = why.into(),
                }
                o
            })
            .collect()
    };
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
        json!({"extent": e, "vertices": t.vertex_count(), "cell_m": t.cell_m, "layers": t.layers.iter().map(|l| l.material.clone()).collect::<Vec<_>>(), "auto": t.auto, "hex_tiling": t.hex_tiling, "cover": t.cover})
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
    out.insert("underlays".into(), underlays.into());
    out.insert("wall_photos".into(), wall_photos.into());
    out.insert("viewpoints".into(), json!(p.viewpoints));
    out.insert("films".into(), json!(p.films));
    out.insert("terrain".into(), terrain.into());
    out.insert("rocks".into(), json!(p.rocks));
    out.insert("deadwood".into(), json!(p.deadwood));
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

/// Plants as rays see them: for each plant of the visible plantings the crown and trunk
/// `years` after planting (`None`: full grown), nearest hit of `dir` from `origin`.
fn cast_plants(
    project: &Project,
    owners: &[Owner],
    assets: &[visioncraft_core::Asset],
    years: Option<f32>,
    origin: [f32; 3],
    dir: [f32; 3],
) -> Option<Value> {
    let mut best: Option<(f32, Value)> = None;
    for g in &project.generators {
        if !project.layer_visible(g.layer()) {
            continue;
        }
        for (i, pl) in placements(project, owners, assets, g).0.iter().enumerate() {
            let gr = visioncraft_core::growth::growth_of(&pl.asset, None);
            let age = match years {
                Some(y) => g.planted_age_years().unwrap_or(gr.planted_age_years) + y,
                None => gr.years_to_maturity * 4.0,
            };
            let solids =
                visioncraft_core::raycast::plant_solids(&pl.asset, pl.position, pl.scale, age);
            let Some((t, part)) = solids.hit(origin, dir) else {
                continue;
            };
            if best.as_ref().is_some_and(|(d, _)| *d <= t) {
                continue;
            }
            let point = [0, 1, 2].map(|k| origin[k] + dir[k] * t);
            let normal = match (part, solids.crown, solids.trunk) {
                ("crown", Some((c, r)), _) => {
                    let n = [0, 1, 2].map(|k| (point[k] - c[k]) / (r[k] * r[k]));
                    let l = n.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
                    n.map(|v| v / l)
                }
                (_, _, Some((f, _, h))) if point[1] >= f[1] + h - 1e-3 => [0.0, 1.0, 0.0],
                (_, _, Some((f, _, _))) => {
                    let n = [point[0] - f[0], 0.0, point[2] - f[2]];
                    let l = n[0].hypot(n[2]).max(1e-9);
                    [n[0] / l, 0.0, n[2] / l]
                }
                _ => [0.0, 1.0, 0.0],
            };
            best = Some((
                t,
                json!({
                    "owner": format!("{}#{i}", g.id()), "kind": "plant", "part": part,
                    "planting": g.id(), "name": g.name(), "asset": pl.asset,
                    "face": null, "point": point, "normal": normal, "distance": t,
                }),
            ));
        }
    }
    best.map(|(_, v)| v)
}

pub(crate) fn raycast_json(
    session: &Session,
    state: &HostState,
    origin: [f32; 3],
    dir: [f32; 3],
    plants: Option<Option<f32>>,
) -> Value {
    let p = session.project();
    let owners = owners(p, &models_of(state));
    let mut hit = cast(p, &owners, origin, dir);
    if let Some(years) = plants
        && let Some(found) = cast_plants(p, &owners, &state.catalog.assets, years, origin, dir)
        && hit.as_ref().is_none_or(|h| {
            found["distance"].as_f64().unwrap_or(f64::MAX)
                < h["distance"].as_f64().unwrap_or(f64::MAX)
        })
    {
        hit = Some(found);
    }
    match hit {
        Some(mut hit) => {
            hit["hit"] = true.into();
            hit
        }
        None => json!({"hit": false}),
    }
}

/// References and routes, over what the host knows: model geometry, plants placed with the
/// building rule and the ground they stand on.
pub(crate) fn with_scene<R>(
    session: &Session,
    state: &HostState,
    years: Option<f32>,
    f: impl FnOnce(&visioncraft_core::reference::Scene) -> R,
) -> R {
    let p = session.project();
    let owners = owners(p, &models_of(state));
    let assets = &state.catalog.assets;
    let ground = |x: f32, z: f32| ground_at(p, &owners, x, z, true).unwrap_or(0.0);
    let place = |g: &Generator| placements(p, &owners, assets, g).0;
    let bounds = |id: &str| {
        owners
            .iter()
            .find(|o| o.kind == "model" && o.id == id)
            .and_then(|o| world_bounds(&o.mesh, o.xf.as_ref()))
    };
    let scene = visioncraft_core::reference::Scene {
        project: p,
        years,
        model_bounds: Some(&bounds),
        placements: Some(&place),
        ground: Some(&ground),
    };
    f(&scene)
}

/// A command's params with references resolved and routes planned (`directing`).
pub(crate) fn resolve_params(
    session: &Session,
    state: &HostState,
    id: &str,
    params: &Value,
) -> Result<(Value, visioncraft_core::directing::Notes), String> {
    let years = visioncraft_core::directing::years_of(session.project(), id, params);
    with_scene(session, state, years, |scene| {
        visioncraft_core::directing::resolve_command(id, params, scene)
    })
}

/// What `query.clearance` was asked.
pub(crate) struct ClearanceQuery<'a> {
    pub from: &'a Value,
    pub to: &'a Value,
    pub clearance_m: Option<f32>,
    pub eye_height_m: f32,
    pub years: Option<f32>,
}

/// `query.clearance`: whether one can walk and see from one place to another.
pub(crate) fn clearance_json(
    session: &Session,
    state: &HostState,
    q: &ClearanceQuery,
) -> Result<Value, String> {
    use visioncraft_core::reference::{place, place_xz};
    use visioncraft_core::route::{self, BODY_RADIUS_M, Obstacles};
    let years = q.years.or(session.project().view.years);
    let (from, to, obstacles) = with_scene(session, state, years, |scene| {
        Ok::<_, String>((
            place_xz(scene, q.from, "from")?,
            place_xz(scene, q.to, "to")?,
            Obstacles::from_scene(scene),
        ))
    })?;
    let margin = q.clearance_m.unwrap_or(BODY_RADIUS_M);
    let line = [from, to];
    let length = (to[0] - from[0]).hypot(to[1] - from[1]);
    let crossings = obstacles.crossings(&line, margin);
    // The nearest an obstacle comes to the line, sampled every 0.25 m.
    let steps = (length / 0.25).ceil().max(1.0) as usize;
    let nearest = (0..=steps)
        .filter_map(|i| {
            let t = i as f32 / steps as f32;
            let p = [
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
            ];
            obstacles.nearest(p).map(|(w, d)| (w, d, p))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1));
    let mut out = json!({
        "from": from, "to": to, "length_m": length, "clearance_m": margin,
        "clear": crossings.is_empty(),
        "crossings": crossings.iter().map(|c| json!({"what": c.what, "at": c.at, "distance_m": c.distance_m})).collect::<Vec<_>>(),
        "nearest": nearest.map(|(w, d, p)| json!({"what": w, "distance_m": d, "at": p})),
    });
    if !crossings.is_empty() {
        let planned = route::plan(&obstacles, &line, q.clearance_m);
        let len: f32 = planned
            .path
            .windows(2)
            .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
            .sum();
        out["route"] = json!({"path": planned.path, "length_m": len, "warnings": planned.warnings});
    }
    // Sight: an eye-high ray, over the plants' crowns and trunks too.
    let p = session.project();
    let ground =
        |x: f32, z: f32| ground_at(p, &owners(p, &models_of(state)), x, z, true).unwrap_or(0.0);
    let eye = |q2: [f32; 2]| [q2[0], ground(q2[0], q2[1]) + q.eye_height_m, q2[1]];
    let (a, b) = (eye(from), eye(to));
    let dir = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let hit = raycast_json(session, state, a, dir, Some(years));
    let blocked = hit["hit"] == true && hit["distance"].as_f64().is_some_and(|d| d < 0.999);
    out["sight"] = if blocked {
        json!({"clear": false, "blocked_by": hit["owner"], "kind": hit["kind"], "at": hit["point"]})
    } else {
        json!({"clear": true})
    };
    let _ = place;
    Ok(out)
}

pub(crate) fn ground_json(session: &Session, state: &HostState, x: f32, z: f32) -> Value {
    let p = session.project();
    json!({"x": x, "z": z, "height": ground_at(p, &owners(p, &models_of(state)), x, z, true)})
}

/// A film's cameras over the ground of the project (terrain, areas, models).
pub(crate) fn film_timeline(
    session: &Session,
    state: &HostState,
    film: &visioncraft_core::film::Film,
) -> Result<visioncraft_core::film::Timeline, String> {
    let p = session.project();
    let owners = owners(p, &models_of(state));
    let ground = |x: f32, z: f32| ground_at(p, &owners, x, z, true).unwrap_or(0.0);
    visioncraft_core::film::Timeline::new(film, &visioncraft_core::film::Env::new(p, &ground))
}

/// `query.ground_length`: a walk over the ground.
pub(crate) fn ground_length_json(session: &Session, state: &HostState, path: &[[f32; 2]]) -> Value {
    let p = session.project();
    let owners = owners(p, &models_of(state));
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

use visioncraft_core::SunMap;
use visioncraft_core::sun_hours::DaySun;
use visioncraft_core::sun_map::{GridSpec, MAX_CELLS, SunMapStats};

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
/// cannot reach), the terrain and the plant crowns. Owns what it needs, so a job can move to
/// another thread.
struct Shadows {
    project: Project,
    owners: Vec<Owner>,
    /// The owners a ray can hit: (index into `owners`, world box).
    casters: Vec<(usize, [[f32; 3]; 2])>,
    crowns: Vec<Occluder>,
    /// The terraces, whose footprints are cut out of the terrain.
    zones: Vec<visioncraft_core::terrace::Zone>,
}

impl Shadows {
    /// The shadow casters of `project` with the crowns of its plants as they are in `month`
    /// `years` after planting (none when `plants` is false).
    fn new(
        project: &Project,
        models: &Models,
        assets: &[visioncraft_core::Asset],
        month: u32,
        years: f32,
        plants: bool,
    ) -> Self {
        let owners = owners(project, models);
        let crowns = if plants {
            plant_crowns(project, &owners, assets, month, years)
        } else {
            Vec::new()
        };
        Self::with_crowns(project, owners, crowns)
    }

    /// The shadow casters of `project` with these crowns.
    fn with_crowns(project: &Project, owners: Vec<Owner>, crowns: Vec<Occluder>) -> Self {
        let casters = owners
            .iter()
            .enumerate()
            .filter_map(|(i, o)| Some((i, world_bounds(&o.mesh, o.xf.as_ref())?)))
            .collect();
        Self {
            project: project.clone(),
            owners,
            casters,
            crowns,
            zones: visioncraft_core::terrace::project_zones(project),
        }
    }

    /// The height of the ground plants stand on at `(x, z)`.
    fn ground(&self, x: f32, z: f32) -> Option<f32> {
        ground_at(&self.project, &self.owners, x, z, true)
    }

    /// The share (0..=1) of the sun's light that reaches `origin` along `dir`: 0 behind a model,
    /// shape or the terrain, else the product of what the crowns in the way let through, leaving
    /// out the crowns whose index is in `skip`.
    fn light_skipping(&self, origin: [f32; 3], dir: [f32; 3], skip: &Range<usize>) -> f32 {
        for (i, b) in &self.casters {
            if !ray_hits_box(origin, dir, b) {
                continue;
            }
            let o = &self.owners[*i];
            let (lo, ld) = match &o.xf {
                Some(t) => (to_local(t, origin, true), to_local(t, dir, false)),
                None => (origin, dir),
            };
            if raycast_any(&o.mesh, lo, ld) {
                return 0.0;
            }
        }
        if let Some(t) = &self.project.terrain
            && visioncraft_core::terrace::terrain_raycast(t, &self.zones, origin, dir).is_some()
        {
            return 0.0;
        }
        self.crowns
            .iter()
            .enumerate()
            .filter(|(i, _)| !skip.contains(i))
            .map(|(_, c)| c.pass(origin, dir))
            .product()
    }

    /// The light at a spot `height_m` over the ground for every slice of the day.
    fn lit_at(&self, day: &DaySun, x: f32, z: f32, height_m: f32) -> Vec<(SunSample, f32)> {
        self.lit_at_skipping(day, x, z, height_m, &(0..0))
    }

    fn lit_at_skipping(
        &self,
        day: &DaySun,
        x: f32,
        z: f32,
        height_m: f32,
        skip: &Range<usize>,
    ) -> Vec<(SunSample, f32)> {
        let y = self.ground(x, z).unwrap_or(0.0) + height_m;
        // A little above the sample so the ray does not start in its own ground.
        let origin = [x, y + 0.01, z];
        day.samples
            .iter()
            .map(|s| (*s, self.light_skipping(origin, s.direction, skip)))
            .collect()
    }
}

fn hours_of(lit: &[(SunSample, f32)]) -> f64 {
    lit.iter().map(|(s, l)| s.hours() * f64::from(*l)).sum()
}

/// What a grid of sun hours is made of.
pub struct SunGridQuery {
    pub spec: GridSpec,
    /// The cell centres inside the area and not under another area, `None` otherwise
    /// (`sun_map::clear_cells`).
    pub cells: Vec<Option<[f32; 2]>>,
    pub month: u32,
    pub day: u32,
    /// Years after planting the crowns have grown.
    pub years: f32,
    /// The height over the ground the sun is measured at.
    pub height_m: f32,
    pub step_minutes: f32,
    pub plants: bool,
}

/// Hours of sun over a grid, cast a few cells at a time: a host that must not stop (a window
/// on the web) calls [`SunGridJob::step`] every frame, one that has a thread runs it at once.
/// The same computation `query.sun_hours` does for an outline.
pub struct SunGridJob {
    shadows: Shadows,
    day: DaySun,
    q: SunGridQuery,
    hours: Vec<Option<f64>>,
    next: usize,
}

/// A finished grid.
#[derive(Clone, Debug)]
pub struct SunGrid {
    pub spec: GridSpec,
    /// Hours of sun per cell, row-major from the lowest z (the north side); `None` outside.
    pub hours: Vec<Option<f64>>,
    pub month: u32,
    pub day: u32,
    pub daylight_hours: f64,
    pub years: f32,
    pub crowns: usize,
}

impl SunGrid {
    /// The numbers for a legend; `None` when no cell is inside.
    pub fn stats(&self) -> Option<SunMapStats> {
        SunMapStats::of(self.month, self.day, self.daylight_hours, &self.hours)
    }
}

impl SunGridJob {
    /// `models` gives the geometry of the project's models by id, `assets` the library's
    /// plants (for their size).
    pub fn new(
        project: &Project,
        models: &Models,
        assets: &[visioncraft_core::Asset],
        q: SunGridQuery,
    ) -> Self {
        let day = day_sun(
            &project.site,
            project.time.year,
            q.month,
            q.day,
            f64::from(q.step_minutes),
        );
        let shadows = Shadows::new(project, models, assets, q.month, q.years, q.plants);
        Self {
            shadows,
            day,
            hours: Vec::with_capacity(q.cells.len()),
            q,
            next: 0,
        }
    }

    /// The job for a sun map setting, checked against the project (`SunMap::validate`).
    pub fn for_map(
        project: &Project,
        models: &Models,
        assets: &[visioncraft_core::Asset],
        map: &SunMap,
    ) -> Result<Self, String> {
        map.validate(project)?;
        let outline = map.resolve_outline(project)?;
        let spec = GridSpec::new(&outline, map.cell_m);
        let (month, day) = map.day(project);
        Ok(Self::new(
            project,
            models,
            assets,
            SunGridQuery {
                cells: visioncraft_core::sun_map::clear_cells(
                    project,
                    &spec,
                    &outline,
                    map.shape.as_deref(),
                ),
                spec,
                month,
                day,
                years: map.years_for(project),
                height_m: 0.05,
                step_minutes: 10.0,
                plants: true,
            },
        ))
    }

    /// Cells in the grid, and how many are done.
    pub fn progress(&self) -> (usize, usize) {
        (self.next, self.q.cells.len())
    }

    /// Does the next `n` cells; true when all are done.
    pub fn step(&mut self, n: usize) -> bool {
        let end = self.next.saturating_add(n).min(self.q.cells.len());
        for c in &self.q.cells[self.next..end] {
            self.hours.push(
                c.map(|[x, z]| hours_of(&self.shadows.lit_at(&self.day, x, z, self.q.height_m))),
            );
        }
        self.next = end;
        self.next == self.q.cells.len()
    }

    /// The grid; the cells not done yet count as outside.
    pub fn finish(mut self) -> SunGrid {
        self.hours.resize(self.q.cells.len(), None);
        SunGrid {
            spec: self.q.spec,
            hours: self.hours,
            month: self.q.month,
            day: self.q.day,
            daylight_hours: self.day.daylight_hours,
            years: self.q.years,
            crowns: self.shadows.crowns.len(),
        }
    }
}

/// One plant of the project as the sun queries see it.
struct PlantAt {
    generator: String,
    name: String,
    /// Its index among the planting's placements.
    index: usize,
    asset: String,
    position: [f32; 3],
    scale: f32,
    /// Its age (years) at the time looked at: the planted age plus `years`.
    age_years: f32,
    /// The crowns of its planting, as indexes into the crown list (the crowns of one planting
    /// are next to each other).
    crowns: Range<usize>,
}

/// The visible plantings' plants and the crowns of the woody ones `years` after planting, as
/// ellipsoids.
fn plant_entries(
    project: &Project,
    owners: &[Owner],
    assets: &[visioncraft_core::Asset],
    month: u32,
    years: f32,
) -> (Vec<PlantAt>, Vec<Occluder>) {
    let (mut plants, mut crowns) = (Vec::new(), Vec::new());
    for g in &project.generators {
        if !project.layer_visible(g.layer()) {
            continue;
        }
        let first = crowns.len();
        let mut mine = Vec::new();
        for (index, pl) in placements(project, owners, assets, g)
            .0
            .into_iter()
            .enumerate()
        {
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
            crowns.extend(plant_occluder(
                &pl.asset,
                pl.position,
                pl.scale,
                age,
                evergreen,
                month,
                project.site.latitude,
            ));
            mine.push(PlantAt {
                generator: g.id().to_string(),
                name: g.name().to_string(),
                index,
                asset: pl.asset,
                position: pl.position,
                scale: pl.scale,
                age_years: age,
                crowns: 0..0,
            });
        }
        let range = first..crowns.len();
        plants.extend(mine.into_iter().map(|mut p| {
            p.crowns = range.clone();
            p
        }));
    }
    (plants, crowns)
}

/// The crowns of the project's woody plants `years` after planting, as ellipsoids.
fn plant_crowns(
    project: &Project,
    owners: &[Owner],
    assets: &[visioncraft_core::Asset],
    month: u32,
    years: f32,
) -> Vec<Occluder> {
    plant_entries(project, owners, assets, month, years).1
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
    let grid = match &q.outline {
        Some(o) => {
            if o.len() < 3 {
                return Err(format!("{ID}: `outline` needs at least 3 points"));
            }
            if !(0.01..=1000.0).contains(&q.cell_m) {
                return Err(format!("{ID}: `cell_m` must be 0.01..1000"));
            }
            let spec = GridSpec::new(o, q.cell_m);
            let (cols, rows) = (spec.cols, spec.rows);
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
            Some((
                spec,
                visioncraft_core::sun_map::clear_cells(p, &spec, o, None),
            ))
        }
        None => None,
    };
    if let Some((_, cells)) = &grid {
        let n = cells.iter().flatten().count();
        if n > MAX_CELLS {
            return Err(format!(
                "{ID}: the outline has {n} cells at `cell_m` {}, at most {MAX_CELLS} are sampled; use a larger `cell_m` (about {:.1} m would do)",
                q.cell_m,
                q.cell_m * (n as f32 / MAX_CELLS as f32).sqrt() * 1.1
            ));
        }
        if n == 0 {
            return Err(format!(
                "{ID}: no cell centre lies inside the outline; use a smaller `cell_m`"
            ));
        }
    }
    let years = q.years.or(p.view.years).unwrap_or(0.0);
    let date = json!({"month": month, "day": day});
    match (q.point, grid) {
        (Some([x, z]), _) => {
            let day_sun = day_sun(&p.site, year, month, day, f64::from(q.step_minutes));
            let shadows = Shadows::new(
                p,
                &models_of(state),
                &state.catalog.assets,
                month,
                years,
                q.plants,
            );
            let lit = shadows.lit_at(&day_sun, x, z, q.height_m);
            let height = shadows.ground(x, z).unwrap_or(0.0) + q.height_m;
            Ok(json!({
                "point": [x, z], "height": r3(f64::from(height)), "date": date,
                "sunrise_hour": day_sun.sunrise.map(r2), "sunset_hour": day_sun.sunset.map(r2),
                "daylight_hours": r2(day_sun.daylight_hours), "sun_hours": r2(hours_of(&lit)),
                "intervals": lit_intervals(&lit).iter().map(|i| [r2(i[0]), r2(i[1])]).collect::<Vec<_>>(),
                "crowns": shadows.crowns.len(), "years": years,
            }))
        }
        (None, Some((spec, cells))) => {
            let mut job = SunGridJob::new(
                p,
                &models_of(state),
                &state.catalog.assets,
                SunGridQuery {
                    spec,
                    cells,
                    month,
                    day,
                    years,
                    height_m: q.height_m,
                    step_minutes: q.step_minutes,
                    plants: q.plants,
                },
            );
            job.step(usize::MAX);
            let g = job.finish();
            let inside: Vec<f64> = g.hours.iter().flatten().copied().collect();
            let n = inside.len() as f64;
            let share =
                |f: &dyn Fn(f64) -> bool| r3(inside.iter().filter(|h| f(**h)).count() as f64 / n);
            let count = |lo: f64, hi: f64| inside.iter().filter(|h| **h >= lo && **h < hi).count();
            Ok(json!({
                "date": date, "cell_m": q.cell_m, "origin": spec.origin,
                "cols": spec.cols, "rows": spec.rows,
                "hours": g.hours.iter().map(|h| h.map(r2)).collect::<Vec<_>>(),
                "min": r2(inside.iter().copied().fold(f64::INFINITY, f64::min)),
                "max": r2(inside.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
                "mean": r2(inside.iter().sum::<f64>() / n),
                "daylight_hours": r2(g.daylight_hours),
                "histogram": {
                    "0-2": count(0.0, 2.0), "2-4": count(2.0, 4.0),
                    "4-6": count(4.0, 6.0), "6+": count(6.0, f64::INFINITY),
                },
                "full_sun_share": share(&|h| h >= FULL_SUN_HOURS),
                "part_shade_share": share(&|h| (PART_SHADE_HOURS..FULL_SUN_HOURS).contains(&h)),
                "shade_share": share(&|h| h < PART_SHADE_HOURS),
                "cells": inside.len(), "crowns": g.crowns, "years": years,
            }))
        }
        (None, None) => unreachable!("one of point and outline was checked"),
    }
}

/// The day the light queries look at: the date or season given, else the summer solstice (the
/// light ranges are summer-day hours).
fn light_day(
    id: &str,
    p: &Project,
    date: Option<(u32, u32)>,
    season: Option<Season>,
) -> Result<(u32, u32), String> {
    if date.is_some() && season.is_some() {
        return Err(format!("{id}: give `date` or `season`, not both"));
    }
    match date {
        Some((m, d)) => {
            if !(1..=12).contains(&m) || d < 1 || d > days_in_month(p.time.year, m) {
                return Err(format!(
                    "{id}: `date` {{month: {m}, day: {d}}} is not a day of {}",
                    p.time.year
                ));
            }
            Ok((m, d))
        }
        None => Ok(season_date(
            season.unwrap_or(Season::Summer),
            p.site.latitude,
        )),
    }
}

/// What `query.plants_for_light` was asked of a host, checked by the command.
pub(crate) struct PlantsForLightQuery {
    pub point: Option<[f32; 2]>,
    pub outline: Option<Vec<[f32; 2]>>,
    pub date: Option<(u32, u32)>,
    pub season: Option<Season>,
    pub grower: Option<visioncraft_core::growth::Grower>,
    pub limit: usize,
    pub cell_m: f32,
    pub plants: bool,
    pub years: Option<f32>,
}

/// The sun hours of a point or an outline on `date` as (min, mean, max, cells, spot JSON).
fn spot_hours(
    session: &Session,
    state: &HostState,
    id: &str,
    query: SunHoursQuery,
) -> Result<(f32, f32, f32, Value), String> {
    let (point, outline) = (query.point, query.outline.clone());
    let r = sun_hours_json(session, state, &query).map_err(|e| e.replace("query.sun_hours", id))?;
    let f = |k: &str| r[k].as_f64().unwrap_or(0.0) as f32;
    let mut spot = json!({
        "date": r["date"], "daylight_hours": r["daylight_hours"],
        "crowns": r["crowns"], "years": r["years"],
    });
    let (min, mean, max) = if let Some(p) = point {
        let h = f("sun_hours");
        spot["point"] = json!(p);
        (h, h, h)
    } else {
        spot["outline"] = json!(outline);
        spot["cells"] = r["cells"].clone();
        (f("min"), f("mean"), f("max"))
    };
    spot["min"] = json!(r2(f64::from(min)));
    spot["mean"] = json!(r2(f64::from(mean)));
    spot["max"] = json!(r2(f64::from(max)));
    Ok((min, mean, max, spot))
}

fn add_names(list: &mut Value) {
    if let Some(v) = list.as_array_mut() {
        for e in v {
            let name = e["species"]
                .as_str()
                .and_then(|s| s.strip_prefix("flora:"))
                .and_then(visioncraft_flora::Species::by_id)
                .map(|s| s.common_name);
            if let Some(n) = name {
                e["name"] = json!(n);
            }
        }
    }
}

/// `query.plants_for_light` for a point or an outline: the sun hours of the spot, then the
/// ranking of `visioncraft_core::plant_light`.
pub(crate) fn plants_for_light_json(
    session: &Session,
    state: &HostState,
    q: &PlantsForLightQuery,
) -> Result<Value, String> {
    const ID: &str = "query.plants_for_light";
    if q.point.is_some() == q.outline.is_some() {
        return Err(format!(
            "{ID}: give exactly one of `hours`, `point` and `outline`"
        ));
    }
    let day = light_day(ID, session.project(), q.date, q.season)?;
    let (_, mean, _, spot) = spot_hours(
        session,
        state,
        ID,
        SunHoursQuery {
            point: q.point,
            outline: q.outline.clone(),
            height_m: 0.05,
            date: Some(day),
            season: None,
            step_minutes: 10.0,
            plants: q.plants,
            years: q.years,
            cell_m: q.cell_m,
        },
    )?;
    let mut out =
        visioncraft_core::plant_light::plants_for_light_json(spot, mean, q.grower, q.limit);
    add_names(&mut out["species"]);
    Ok(out)
}

/// The mean sun hours over a border's outline on the summer solstice, with the spot's numbers.
pub(crate) fn border_light(
    session: &Session,
    state: &HostState,
    outline: &[[f32; 2]],
) -> Result<(f32, Value), String> {
    let (min, max) = outline.iter().fold(
        ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]),
        |(lo, hi), p| {
            (
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            )
        },
    );
    // About 400 cells at most, but no finer than half a metre.
    let area = ((max[0] - min[0]) * (max[1] - min[1])).max(0.01);
    let mut cell_m = (area / 400.0).sqrt().max(0.5);
    // A thin outline may have no cell centre inside: refine until one has.
    loop {
        match spot_hours(
            session,
            state,
            "query.plants_for_light",
            SunHoursQuery {
                point: None,
                outline: Some(outline.to_vec()),
                height_m: 0.05,
                date: Some(light_day(
                    "query.plants_for_light",
                    session.project(),
                    None,
                    None,
                )?),
                season: None,
                step_minutes: 10.0,
                plants: true,
                years: None,
                cell_m,
            },
        ) {
            Ok((_, mean, _, spot)) => return Ok((mean, spot)),
            Err(e) if e.contains("no cell centre") && cell_m > 0.02 => cell_m /= 2.0,
            Err(e) => return Err(e),
        }
    }
}

/// What `query.light_check` was asked, checked by the command.
pub struct LightCheckQuery {
    pub date: Option<(u32, u32)>,
    pub season: Option<Season>,
    pub years: Option<f32>,
    pub plants: bool,
    pub step_minutes: f32,
    pub ids: Option<Vec<String>>,
    pub layer: Option<String>,
    pub suggestions: usize,
    pub include_ok: bool,
    pub limit: usize,
}

impl Default for LightCheckQuery {
    /// The summer solstice, the view's years, the crowns on, 3 suggestions, 200 listed.
    fn default() -> Self {
        Self {
            date: None,
            season: None,
            years: None,
            plants: true,
            step_minutes: 15.0,
            ids: None,
            layer: None,
            suggestions: 3,
            include_ok: false,
            limit: 200,
        }
    }
}

/// Lawn tufts are many and alike: this many per lawn are measured.
const LAWN_SAMPLES: usize = 25;
/// More plants than this in one check are refused (narrow it with `ids` or `layer`).
const MAX_CHECKED: usize = 4000;

/// `query.light_check` on a host.
pub(crate) fn light_check_json(
    session: &Session,
    state: &HostState,
    q: &LightCheckQuery,
) -> Result<Value, String> {
    light_check(
        session.project(),
        &models_of(state),
        &state.catalog.assets,
        q,
    )
}

/// `query.light_check`: the sun hours at every placed plant of `p`, against the species' light
/// range. `models` gives the geometry of the project's models by id, `assets` the library's
/// plants (for their size).
pub fn light_check(
    p: &Project,
    models: &Models,
    assets: &[visioncraft_core::Asset],
    q: &LightCheckQuery,
) -> Result<Value, String> {
    use visioncraft_core::plant_light::{Verdict, light_of, suggest, verdict};
    const ID: &str = "query.light_check";
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
    if let Some(ids) = &q.ids {
        for id in ids {
            if !p.generators.iter().any(|g| g.id() == id) {
                return Err(format!("{ID}: no planting `{id}`"));
            }
        }
    }
    if let Some(l) = &q.layer
        && !p.layers.iter().any(|x| &x.id == l)
    {
        return Err(format!("{ID}: no layer `{l}`"));
    }
    let (month, day_of_month) = light_day(ID, p, q.date, q.season)?;
    let years = q.years.or(p.view.years).unwrap_or(0.0);
    let day = day_sun(
        &p.site,
        p.time.year,
        month,
        day_of_month,
        f64::from(q.step_minutes),
    );
    let own = owners(p, models);
    let (mut plants, crowns) = if q.plants {
        plant_entries(p, &own, assets, month, years)
    } else {
        // The plants are wanted, the crowns are not.
        let (plants, _) = plant_entries(p, &own, assets, month, years);
        (plants, Vec::new())
    };
    let crown_count = crowns.len();
    let shadows = Shadows::with_crowns(p, own, crowns);
    // What is checked: flora plants of the planting or layer asked for; lawns sampled.
    let wanted = |pl: &PlantAt| {
        pl.asset.starts_with("flora:")
            && q.ids.as_ref().is_none_or(|ids| ids.contains(&pl.generator))
            && q.layer.as_ref().is_none_or(|l| {
                p.generators
                    .iter()
                    .any(|g| g.id() == pl.generator && g.layer() == l)
            })
    };
    plants.retain(|pl| wanted(pl));
    let mut lawn_seen: HashMap<String, usize> = HashMap::new();
    let mut lawn_total: HashMap<String, usize> = HashMap::new();
    for pl in &plants {
        if visioncraft_core::growth::growth_of(&pl.asset, None).grower
            == visioncraft_core::growth::Grower::Lawn
        {
            *lawn_total.entry(pl.generator.clone()).or_default() += 1;
        }
    }
    let mut sampled_out = 0usize;
    plants.retain(|pl| {
        let is_lawn = visioncraft_core::growth::growth_of(&pl.asset, None).grower
            == visioncraft_core::growth::Grower::Lawn;
        if !is_lawn {
            return true;
        }
        let total = lawn_total[&pl.generator];
        let stride = total.div_ceil(LAWN_SAMPLES).max(1);
        let n = lawn_seen.entry(pl.generator.clone()).or_default();
        let keep = n.is_multiple_of(stride);
        *n += 1;
        sampled_out += usize::from(!keep);
        keep
    });
    if plants.len() > MAX_CHECKED {
        return Err(format!(
            "{ID}: {} plants to check, at most {MAX_CHECKED}; narrow it with `ids` or `layer`",
            plants.len()
        ));
    }
    let name = |asset: &str| {
        asset
            .strip_prefix("flora:")
            .and_then(visioncraft_flora::Species::by_id)
            .map(|s| s.common_name)
    };
    let mut counts = [0usize; 3];
    let mut by_planting: Vec<(String, String, [usize; 3])> = Vec::new();
    let mut by_species: HashMap<String, (usize, usize, usize, f64)> = HashMap::new();
    let mut listed: Vec<Value> = Vec::new();
    let mut mismatches = 0usize;
    let mut suggestion_cache: HashMap<(String, i32), Vec<Value>> = HashMap::new();
    for pl in &plants {
        let need = light_of(&pl.asset, None);
        let gr = visioncraft_core::growth::growth_of(&pl.asset, None);
        let height = gr.mature_height_m * gr.size(pl.age_years).height * pl.scale;
        // Mid-height of the plant, up to 2 m: a tree is judged by its middle, not its trunk's foot.
        let at = (height * 0.5).clamp(0.05, 2.0);
        let [x, y, z] = pl.position;
        let _ = y;
        let lit = shadows.lit_at_skipping(&day, x, z, at, &pl.crowns);
        let hours = hours_of(&lit) as f32;
        let v = verdict(&need.light(), hours);
        let k = match v {
            Verdict::Ok => 0,
            Verdict::TooSunny => 1,
            Verdict::TooShady => 2,
        };
        counts[k] += 1;
        match by_planting.iter_mut().find(|b| b.0 == pl.generator) {
            Some(b) => b.2[k] += 1,
            None => {
                let mut c = [0; 3];
                c[k] = 1;
                by_planting.push((pl.generator.clone(), pl.name.clone(), c));
            }
        }
        let sp = by_species.entry(pl.asset.clone()).or_default();
        match k {
            0 => sp.0 += 1,
            1 => sp.1 += 1,
            _ => sp.2 += 1,
        }
        sp.3 += f64::from(hours);
        if k == 0 && !q.include_ok {
            continue;
        }
        if k != 0 {
            mismatches += 1;
        }
        if listed.len() >= q.limit {
            continue;
        }
        let key = (pl.asset.clone(), (hours * 2.0).round() as i32);
        let suggestions = if k == 0 || q.suggestions == 0 {
            Vec::new()
        } else {
            suggestion_cache
                .entry(key)
                .or_insert_with(|| {
                    let mut v: Vec<Value> = suggest(&pl.asset, hours, q.suggestions)
                        .iter()
                        .map(|r| {
                            let mut e = json!({
                                "species": format!("flora:{}", r.id), "fit": r.fit,
                                "light": {"min_hours": r.light.min_hours, "max_hours": r.light.max_hours},
                            });
                            if let Some(n) = name(&format!("flora:{}", r.id)) {
                                e["name"] = json!(n);
                            }
                            e
                        })
                        .collect();
                    v.shrink_to_fit();
                    v
                })
                .clone()
        };
        let mut e = json!({
            "id": format!("{}#{}", pl.generator, pl.index),
            "planting": pl.generator, "species": pl.asset,
            "position": [r2(f64::from(x)), r2(f64::from(z))],
            "hours": r2(f64::from(hours)),
            "light": {"min_hours": need.min_hours, "max_hours": need.max_hours},
            "from_table": need.from_table, "estimated": need.estimated,
            "verdict": v, "suggestions": suggestions,
        });
        if let Some(n) = name(&pl.asset) {
            e["name"] = json!(n);
        }
        listed.push(e);
    }
    let mut species: Vec<Value> = by_species
        .iter()
        .map(|(asset, (ok, sunny, shady, total))| {
            let n = (ok + sunny + shady) as f64;
            let need = light_of(asset, None);
            json!({
                "species": asset, "name": name(asset), "count": ok + sunny + shady,
                "ok": ok, "too_sunny": sunny, "too_shady": shady,
                "mean_hours": r2(total / n),
                "light": {"min_hours": need.min_hours, "max_hours": need.max_hours},
            })
        })
        .collect();
    species.sort_by(|a, b| {
        let m =
            |v: &Value| v["too_sunny"].as_u64().unwrap_or(0) + v["too_shady"].as_u64().unwrap_or(0);
        m(b).cmp(&m(a))
            .then(a["species"].as_str().cmp(&b["species"].as_str()))
    });
    Ok(json!({
        "date": {"month": month, "day": day_of_month}, "years": years,
        "daylight_hours": r2(day.daylight_hours), "crowns": crown_count,
        "summary": {
            "checked": plants.len(), "ok": counts[0], "too_sunny": counts[1], "too_shady": counts[2],
            "lawn_tufts_not_measured": sampled_out,
        },
        "mismatches": mismatches,
        "listed": listed.len(),
        "truncated": mismatches + if q.include_ok { counts[0] } else { 0 } > listed.len(),
        "plants": listed,
        "by_planting": by_planting.iter().map(|(id, name, c)| json!({
            "id": id, "name": name, "ok": c[0], "too_sunny": c[1], "too_shady": c[2],
        })).collect::<Vec<_>>(),
        "by_species": species,
        "note": "light ranges are estimated from general horticultural knowledge, not yet checked against open data; a plant is measured at half its height (up to 2 m), the plants of its own planting do not shade it, a verdict allows 0.5 h either side of the range",
    }))
}

/// The age a plant is drawn at when the view shows every plant full grown (`ViewState::years`
/// `None`): far past any growth curve's maturity.
const FULL_GROWN_YEARS: f32 = 500.0;

/// What blocks the sky round `project` for the sky visibility (`visioncraft_core::sky_visibility`,
/// `query.sky_view`): the visible models' and shapes' triangles less glass and see-through
/// materials (areas are the ground, not occluders), the crowns of the woody plants `years` after
/// planting (`None`: full grown, as the window draws them) with their species' leaf density in
/// `month`, and the ground (terrain and terraces, else the ground plane).
pub fn sky_scene(
    project: &Project,
    models: &Models,
    assets: &[visioncraft_core::Asset],
    years: Option<f32>,
    month: u32,
) -> visioncraft_core::SkyScene {
    use visioncraft_core::sky_visibility::{Canopy, Ground, SkyScene, crown_extinction};
    let owners = owners(project, models);
    let mut triangles = Vec::new();
    for o in owners.iter().filter(|o| !(o.ground && o.kind == "shape")) {
        let materials = visioncraft_core::resolve_materials(project, &o.id, &o.mesh);
        let clear = |face: u32| {
            materials
                .get(face as usize)
                .copied()
                .flatten()
                .and_then(|m| project.materials.get(m))
                .is_some_and(|m| {
                    matches!(
                        m.kind,
                        visioncraft_core::SurfaceKind::ArchitecturalGlass { .. }
                    ) || m.base_color[3] < 0.99
                })
        };
        for (t, face) in o.mesh.triangles.iter().zip(&o.mesh.triangle_faces) {
            if clear(*face) {
                continue;
            }
            triangles.push(t.map(|v| {
                let p = o.mesh.positions[v as usize];
                o.xf.as_ref().map_or(p, |x| to_world(x, p))
            }));
        }
    }
    let mut crowns = Vec::new();
    for g in &project.generators {
        if !project.layer_visible(g.layer()) {
            continue;
        }
        for pl in placements(project, &owners, assets, g).0 {
            let evergreen = pl
                .asset
                .strip_prefix("flora:")
                .and_then(visioncraft_flora::Species::by_id)
                .is_none_or(|s| s.evergreen);
            let age = g.planted_age_years().unwrap_or_else(|| {
                visioncraft_core::growth::growth_of(&pl.asset, None).planted_age_years
            }) + years.unwrap_or(FULL_GROWN_YEARS);
            let latitude = project.site.latitude;
            if let Some(c) = plant_occluder(
                &pl.asset,
                pl.position,
                pl.scale,
                age,
                evergreen,
                month,
                latitude,
            ) {
                let leaves = visioncraft_core::sun_hours::leaf_share(evergreen, month, latitude);
                crowns.push(Canopy {
                    center: c.center,
                    radii: c.radii,
                    extinction: crown_extinction(&pl.asset, c.radii[1], leaves),
                });
            }
        }
    }
    let plane = project.view.ground.map_or(0.0, |g| g.height);
    let ground = if project.terrain.is_some() {
        let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        let mut grow = |x: f32, z: f32, r: f32| {
            lo = [lo[0].min(x - r), lo[1].min(z - r)];
            hi = [hi[0].max(x + r), hi[1].max(z + r)];
        };
        triangles
            .iter()
            .flatten()
            .for_each(|p| grow(p[0], p[2], 0.0));
        crowns
            .iter()
            .for_each(|c| grow(c.center[0], c.center[2], c.radii[0].max(c.radii[2])));
        if lo[0].is_finite() {
            // Room for the hills round the design that hide the low sky.
            let pad = 30.0;
            let at = visioncraft_core::terrace::ground_fn(project);
            Ground::sample(
                [lo[0] - pad, lo[1] - pad],
                [hi[0] + pad, hi[1] + pad],
                1.0,
                |x, z| at(x, z).unwrap_or(plane),
            )
        } else {
            Ground::flat(plane)
        }
    } else {
        Ground::flat(plane)
    };
    SkyScene {
        triangles,
        crowns,
        ground,
    }
}

/// The sky visibility of a project, baked a few probes at a time (`visioncraft_core::SkyBake`):
/// what the window draws with `view.set sky_occlusion`. A host that must not stop (a window on
/// the web) calls [`SkyVisJob::step`] every frame, one that has a thread runs it at once.
pub struct SkyVisJob {
    bake: Option<visioncraft_core::SkyBake>,
    /// The scene's key (`SkyScene::key`): the same key, the same bake.
    pub key: u64,
}

impl SkyVisJob {
    /// The job for `scene` (`sky_scene`).
    pub fn new(scene: visioncraft_core::SkyScene) -> Self {
        let key = scene.key();
        Self {
            bake: visioncraft_core::SkyBake::new(scene),
            key,
        }
    }

    /// Probes in the grid, and how many are done.
    pub fn progress(&self) -> (usize, usize) {
        self.bake.as_ref().map_or((0, 0), |b| b.progress())
    }

    /// Does the next `n` probes; true when all are done.
    pub fn step(&mut self, n: usize) -> bool {
        self.bake.as_mut().is_none_or(|b| b.step(n))
    }

    /// The baked visibility at once (on all cores natively); `None` when nothing blocks the sky.
    pub fn run(self) -> Option<visioncraft_core::SkyVisibility> {
        self.bake.map(visioncraft_core::SkyBake::run)
    }

    /// As `run`, on at most `threads` threads (a window keeps cores for its frames).
    pub fn run_on(self, threads: usize) -> Option<visioncraft_core::SkyVisibility> {
        self.bake.map(|b| b.run_on(threads))
    }

    /// Probes `start..end` as floats (`SkyBake::probe_floats`); none when nothing blocks the sky.
    pub fn probe_floats(&self, start: usize, end: usize) -> Vec<f32> {
        self.bake
            .as_ref()
            .map_or_else(Vec::new, |b| b.probe_floats(start, end))
    }

    /// Takes the next probes made elsewhere (`SkyBake::fill`); false when they do not fit.
    pub fn fill(&mut self, floats: &[f32]) -> bool {
        self.bake.as_mut().is_some_and(|b| b.fill(floats))
    }

    /// The visibility; `None` when nothing blocks the sky.
    pub fn finish(self) -> Option<visioncraft_core::SkyVisibility> {
        self.bake.map(visioncraft_core::SkyBake::finish)
    }
}

/// Rays `query.sky_view` traces.
const SKY_VIEW_RAYS: usize = 4096;

/// `query.sky_view`: the share of the sky a spot sees, cosine-weighted (the sky view factor).
pub(crate) fn sky_view_json(
    session: &Session,
    state: &HostState,
    point: &[f32],
    normal: Option<[f32; 3]>,
    years: Option<f32>,
) -> Result<Value, String> {
    const ID: &str = "query.sky_view";
    let p = session.project();
    if let Some(y) = years
        && !(0.0..=visioncraft_core::YEARS_MAX).contains(&y)
    {
        return Err(format!(
            "{ID}: `years` must be 0..{}",
            visioncraft_core::YEARS_MAX
        ));
    }
    let n = normal.unwrap_or([0.0, 1.0, 0.0]);
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if !len.is_finite() || len < 1e-6 {
        return Err(format!("{ID}: `normal` must be a direction, not zero"));
    }
    let n = n.map(|v| v / len);
    let models = models_of(state);
    let at = match *point {
        [x, z] => {
            let owners = owners(p, &models);
            [
                x,
                ground_at(p, &owners, x, z, true).unwrap_or(0.0) + 0.02,
                z,
            ]
        }
        [x, y, z] => [x, y, z],
        _ => return Err(format!("{ID}: `point` must be [x, z] or [x, y, z]")),
    };
    if at.iter().any(|v| !v.is_finite()) {
        return Err(format!("{ID}: `point` must be finite numbers"));
    }
    let years = years.or(p.view.years);
    let scene = sky_scene(p, &models, &state.catalog.assets, years, p.time.month);
    // Lifted a hair off the surface it stands on along the normal.
    let from: [f32; 3] = std::array::from_fn(|k| at[k] + n[k] * 0.02);
    let view = visioncraft_core::sky_visibility::sky_view_at(&scene, from, n, SKY_VIEW_RAYS);
    Ok(json!({
        "point": at.map(|v| r3(f64::from(v))), "normal": n.map(|v| r3(f64::from(v))),
        "sky_view": r3(f64::from(view)),
        "crowns": scene.crowns.len(), "years": years, "month": p.time.month,
    }))
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
