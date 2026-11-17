//! What a design contains, as numbers: areas, lengths, volumes and counts per shape, per planting
//! and in total per material and per asset. Headless; the `query.quantities` command wraps it.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use crate::measure;
use crate::model::ImportedModel;
use crate::plant_ground::{placed_shape_mesh, water_areas};
use crate::planting::{Generator, evaluate, planted_outline};
use crate::project::Project;
use crate::shape::{Shape, area_lifts};

/// Area of every triangle of `model`, in m2.
fn triangle_area(model: &ImportedModel, t: [u32; 3]) -> f64 {
    let p = |i: u32| model.positions[i as usize].map(f64::from);
    let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() / 2.0
}

/// A plant species (`flora:...`), as opposed to a light or object placed by a generator.
fn is_plant(asset: &str) -> bool {
    asset.starts_with("flora:")
}

/// Which shapes and plantings to list; `None`: all.
#[derive(Default)]
pub struct Filter<'a> {
    pub ids: Option<&'a [String]>,
    pub layer: Option<&'a str>,
}

fn path_length(points: &[[f32; 2]], closed: bool) -> f64 {
    let mut pts: Vec<[f32; 3]> = points.iter().map(|p| [p[0], 0.0, p[1]]).collect();
    if closed && let Some(first) = pts.first().copied() {
        pts.push(first);
    }
    f64::from(measure::polyline_length(&pts))
}

fn material_rows(project: &Project, by: &BTreeMap<Option<usize>, f64>) -> Vec<Value> {
    let mut rows: Vec<(Option<&str>, Value)> = by
        .iter()
        .map(|(i, area)| {
            let m = i.and_then(|i| project.materials.get(i));
            (
                m.map(|m| m.id.as_str()),
                json!({"material": m.map(|m| &m.id), "name": m.map(|m| &m.name), "area_m2": area}),
            )
        })
        .collect();
    // By id, the unassigned surface last.
    rows.sort_by(|a, b| match (a.0, b.0) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    rows.into_iter().map(|r| r.1).collect()
}

fn asset_rows(by: &BTreeMap<String, usize>) -> Vec<Value> {
    by.iter()
        .map(|(asset, count)| json!({"asset": asset, "count": count}))
        .collect()
}

/// The quantities of the project's drawn shapes and plantings (not of linked models, which have
/// no geometry in the core). Errors with a plain message on an unknown id or layer, or a shape
/// that cannot be meshed.
pub fn quantities(project: &Project, filter: &Filter) -> Result<Value, String> {
    if let Some(layer) = filter.layer
        && !project.layers.iter().any(|l| l.id == layer)
    {
        return Err(format!("no layer `{layer}`"));
    }
    if let Some(ids) = filter.ids {
        for id in ids {
            let known = project.shapes.iter().any(|s| s.id() == id)
                || project.generators.iter().any(|g| g.id() == id);
            if !known {
                return Err(format!("no shape or planting `{id}`"));
            }
        }
    }
    let wanted = |id: &str, layer: &str| {
        filter.ids.is_none_or(|ids| ids.iter().any(|i| i == id))
            && filter.layer.is_none_or(|l| l == layer)
    };

    let lifts: HashMap<String, f32> = area_lifts(&project.shapes).into_iter().collect();
    let water = water_areas(project);
    let terrain = project.terrain.as_deref();

    let mut shapes = Vec::new();
    let mut by_material: BTreeMap<Option<usize>, f64> = BTreeMap::new();
    let (mut total_plan, mut total_surface, mut total_volume) = (0.0, 0.0, 0.0);
    for shape in project.shapes.iter().filter(|s| wanted(s.id(), s.layer())) {
        let (mesh, _) = placed_shape_mesh(project, shape, &lifts, &water)
            .map_err(|e| format!("shape `{}`: {e}", shape.id()))?;
        let owners = crate::material::resolve_materials(project, shape.id(), &mesh);
        let mut surface = 0.0;
        let mut own: BTreeMap<Option<usize>, f64> = BTreeMap::new();
        for (t, f) in mesh.triangles.iter().zip(&mesh.triangle_faces) {
            let a = triangle_area(&mesh, *t);
            surface += a;
            *own.entry(owners.get(*f as usize).copied().flatten())
                .or_default() += a;
        }
        for (k, a) in &own {
            *by_material.entry(*k).or_default() += a;
        }
        let mut row = json!({
            "id": shape.id(), "name": shape.name(), "layer": shape.layer(),
            "surface_m2": surface, "materials": material_rows(project, &own),
        });
        let o = row.as_object_mut().expect("a row is an object");
        let (plan, volume);
        match shape {
            Shape::Area(a) => {
                plan = f64::from(measure::polygon_area(&a.outline));
                volume = 0.0;
                o.insert("kind".into(), "area".into());
                o.insert("perimeter_m".into(), path_length(&a.outline, true).into());
            }
            Shape::Extrusion(e) if e.closed => {
                plan = f64::from(measure::polygon_area(&e.outline));
                volume = plan * f64::from(e.height);
                o.insert("kind".into(), "extrusion".into());
                o.insert("perimeter_m".into(), path_length(&e.outline, true).into());
            }
            Shape::Extrusion(e) => {
                let length = path_length(&e.outline, false);
                plan = length * f64::from(e.thickness);
                volume = plan * f64::from(e.height);
                o.insert("kind".into(), "extrusion".into());
                o.insert("length_m".into(), length.into());
            }
        }
        o.insert("plan_area_m2".into(), plan.into());
        if let Shape::Extrusion(e) = shape {
            o.insert("height_m".into(), e.height.into());
            o.insert("volume_m3".into(), volume.into());
        }
        total_plan += plan;
        total_surface += surface;
        total_volume += volume;
        shapes.push(row);
    }

    let ground = |x: f32, z: f32| terrain.and_then(|t| t.height_at(x, z)).or(Some(0.0));
    let mut plantings = Vec::new();
    let mut by_asset: BTreeMap<String, usize> = BTreeMap::new();
    let (mut plants, mut placements) = (0, 0);
    for g in project
        .generators
        .iter()
        .filter(|g| wanted(g.id(), g.layer()))
    {
        let e = evaluate(g, &ground);
        let mut own: BTreeMap<String, usize> = BTreeMap::new();
        for p in &e.placements {
            *own.entry(p.asset.clone()).or_default() += 1;
            *by_asset.entry(p.asset.clone()).or_default() += 1;
        }
        // Lights and objects placed along a line are placements too, but not plants.
        let own_plants = e.placements.iter().filter(|p| is_plant(&p.asset)).count();
        plants += own_plants;
        placements += e.placements.len();
        let kind = serde_json::to_value(g)
            .ok()
            .and_then(|v| v.get("type").cloned())
            .unwrap_or(Value::Null);
        let mut row = json!({
            "id": g.id(), "name": g.name(), "layer": g.layer(), "kind": kind,
            "count": e.placements.len(), "plants": own_plants, "truncated": e.truncated,
            "assets": asset_rows(&own),
        });
        let o = row.as_object_mut().expect("a row is an object");
        if let Some(outline) = planted_outline(g) {
            o.insert(
                "area_m2".into(),
                f64::from(measure::polygon_area(&outline)).into(),
            );
        }
        if let Generator::SplineMass(s) = g {
            o.insert("length_m".into(), path_length(&s.points, s.closed).into());
        }
        plantings.push(row);
    }

    Ok(json!({
        "shapes": shapes,
        "plantings": plantings,
        "materials": material_rows(project, &by_material),
        "assets": asset_rows(&by_asset),
        "totals": {
            "plan_area_m2": total_plan, "surface_m2": total_surface,
            "volume_m3": total_volume, "plants": plants, "placements": placements,
        },
    }))
}
