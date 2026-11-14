//! The project as an agent reads it (`document.inspect`), and the queries on its geometry
//! (`query.raycast`, `query.ground_height`, `query.ground_length`), in world space.

use crate::host::{HostState, missing_json};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use visioncraft_core::{
    Generator, ImportedModel, Project, Session, Shape, Transform, area_lifts, evaluate_in_scene,
    evaluate_surface_fill, measure, placed_shape_mesh, plant_size, raycast, water_areas,
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
            let e = evaluate_in_scene(
                g,
                &|x, z| ground_at(project, owners, x, z, true),
                &|x, z| ground_at(project, owners, x, z, false),
                &up,
                &|a| plant_size(project, assets, a, &species),
            );
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

fn bbox(mesh: &ImportedModel, xf: Option<&Transform>) -> Value {
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
    if lo[0].is_finite() {
        json!([lo, hi])
    } else {
        Value::Null
    }
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
            json!({"id": l.id, "name": l.name, "visible": l.visible, "items": items})
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
