use super::{check_layer, fail, merge_update, next_id};
use crate::planting::{
    AreaScatter, Cluster, Generator, MixedBrush, Single, Spacing, SplineMass, Stroke, SurfaceFill,
    Weighted,
};
use crate::project::{LAYER_PLANTING, Project};
use crate::session::{CommandError, Session, bad, parse};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    s.register(
        "planting.add_single",
        "Place one plant: {name, asset, position [x,y,z], rotation_y_deg? (0), scale? (1), layer?}; returns {id}",
        add_single,
    );
    s.register(
        "planting.add_spline",
        "Plants along a curve: {name, asset_palette: [{asset, weight}], points: [[x,z]..], spacing: {count: n} or {every: m}, closed? (false), spacing_jitter? (0), offset_m? (0), scale_min? (1), scale_max? (1), random_rotation? (false), seed?, layer?}; returns {id}",
        add_spline,
    );
    s.register(
        "planting.add_area",
        "Scatter plants in a polygon: {name, asset_palette, outline: [[x,z]..], density_per_m2, position_jitter? (1), scale_min?, scale_max?, random_rotation? (true), min_distance_m? (0), seed?, layer?}; returns {id}",
        add_area,
    );
    s.register(
        "planting.add_brush",
        "Start a brush: {name, asset_palette, density_per_m2, strokes? ([{points, radius_m, erase}]), scale_min?, scale_max?, seed?, layer?}; returns {id}",
        add_brush,
    );
    s.register(
        "planting.brush_stroke",
        "Append a stroke to a brush: {id, points: [[x,z]..], radius_m, erase? (false)}",
        brush_stroke,
    );
    s.register(
        "planting.add_surface_fill",
        "Fill named faces of a model or shape: {name, asset_palette, owner, faces: [name..], density_per_m2, position_jitter? (1), scale_min?, scale_max?, random_rotation? (true), min_distance_m? (0), max_slope_deg? (45), seed?, layer?}; returns {id}",
        add_surface_fill,
    );
    s.register(
        "planting.add_cluster",
        "A clump around a centre: {name, asset_palette, center [x,z], radius_m, count, scale_min?, scale_max?, random_rotation? (true), seed?, layer?}; returns {id}",
        add_cluster,
    );
    s.register(
        "planting.update",
        "Change fields of a generator (any of its fields except id and type): {id, ...}",
        update,
    );
    s.register("planting.remove", "Remove a generator: {id}", remove);
}

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

/// Stores a new generator: `make(id, layer, default seed)`.
fn add(
    project: &mut Project,
    layer: Option<String>,
    make: impl FnOnce(String, String, u64) -> Generator,
) -> Result<Value, CommandError> {
    let layer = layer.unwrap_or_else(|| LAYER_PLANTING.into());
    check_layer(project, &layer)?;
    let id = next_id("g", |id| project.generators.iter().any(|g| g.id() == id));
    // Not every generator the same pattern: the default seed is the generator's number.
    let seed = id[1..].parse().unwrap_or(0);
    let g = make(id.clone(), layer, seed);
    g.validate().map_err(|e| bad(&e))?;
    project.generators.push(g);
    Ok(json!({ "id": id }))
}

fn add_single(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        name: String,
        asset: String,
        position: [f32; 3],
        #[serde(default)]
        rotation_y_deg: f32,
        #[serde(default = "one")]
        scale: f32,
        layer: Option<String>,
    }
    let p: P = parse(params)?;
    add(project, p.layer, |id, layer, _| {
        Generator::Single(Single {
            id,
            name: p.name,
            layer,
            asset: p.asset,
            position: p.position,
            rotation_y_deg: p.rotation_y_deg,
            scale: p.scale,
        })
    })
}

fn add_spline(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        name: String,
        asset_palette: Vec<Weighted>,
        points: Vec<[f32; 2]>,
        spacing: Spacing,
        #[serde(default)]
        closed: bool,
        #[serde(default)]
        spacing_jitter: f32,
        #[serde(default)]
        offset_m: f32,
        #[serde(default = "one")]
        scale_min: f32,
        #[serde(default = "one")]
        scale_max: f32,
        #[serde(default)]
        random_rotation: bool,
        seed: Option<u64>,
        layer: Option<String>,
    }
    let p: P = parse(params)?;
    add(project, p.layer, |id, layer, seed| {
        Generator::SplineMass(SplineMass {
            id,
            name: p.name,
            layer,
            asset_palette: p.asset_palette,
            points: p.points,
            closed: p.closed,
            spacing: p.spacing,
            spacing_jitter: p.spacing_jitter,
            offset_m: p.offset_m,
            scale_min: p.scale_min,
            scale_max: p.scale_max,
            random_rotation: p.random_rotation,
            seed: p.seed.unwrap_or(seed),
        })
    })
}

fn add_area(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        name: String,
        asset_palette: Vec<Weighted>,
        outline: Vec<[f32; 2]>,
        density_per_m2: f32,
        #[serde(default = "one")]
        position_jitter: f32,
        #[serde(default = "one")]
        scale_min: f32,
        #[serde(default = "one")]
        scale_max: f32,
        #[serde(default = "yes")]
        random_rotation: bool,
        #[serde(default)]
        min_distance_m: f32,
        seed: Option<u64>,
        layer: Option<String>,
    }
    let p: P = parse(params)?;
    add(project, p.layer, |id, layer, seed| {
        Generator::AreaScatter(AreaScatter {
            id,
            name: p.name,
            layer,
            asset_palette: p.asset_palette,
            outline: p.outline,
            density_per_m2: p.density_per_m2,
            position_jitter: p.position_jitter,
            scale_min: p.scale_min,
            scale_max: p.scale_max,
            random_rotation: p.random_rotation,
            min_distance_m: p.min_distance_m,
            seed: p.seed.unwrap_or(seed),
        })
    })
}

fn add_brush(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        name: String,
        asset_palette: Vec<Weighted>,
        density_per_m2: f32,
        #[serde(default)]
        strokes: Vec<Stroke>,
        #[serde(default = "one")]
        scale_min: f32,
        #[serde(default = "one")]
        scale_max: f32,
        seed: Option<u64>,
        layer: Option<String>,
    }
    let p: P = parse(params)?;
    add(project, p.layer, |id, layer, seed| {
        Generator::MixedBrush(MixedBrush {
            id,
            name: p.name,
            layer,
            asset_palette: p.asset_palette,
            strokes: p.strokes,
            density_per_m2: p.density_per_m2,
            scale_min: p.scale_min,
            scale_max: p.scale_max,
            seed: p.seed.unwrap_or(seed),
        })
    })
}

fn add_surface_fill(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        name: String,
        asset_palette: Vec<Weighted>,
        owner: String,
        faces: Vec<String>,
        density_per_m2: f32,
        #[serde(default = "one")]
        position_jitter: f32,
        #[serde(default = "one")]
        scale_min: f32,
        #[serde(default = "one")]
        scale_max: f32,
        #[serde(default = "yes")]
        random_rotation: bool,
        #[serde(default)]
        min_distance_m: f32,
        #[serde(default = "slope")]
        max_slope_deg: f32,
        seed: Option<u64>,
        layer: Option<String>,
    }
    fn slope() -> f32 {
        45.0
    }
    let p: P = parse(params)?;
    if !project.models.iter().any(|m| m.id == p.owner)
        && !project.shapes.iter().any(|s| s.id() == p.owner)
    {
        return Err(fail(format!("no model or shape `{}`", p.owner)));
    }
    add(project, p.layer, |id, layer, seed| {
        Generator::SurfaceFill(SurfaceFill {
            id,
            name: p.name,
            layer,
            asset_palette: p.asset_palette,
            owner: p.owner,
            faces: p.faces,
            density_per_m2: p.density_per_m2,
            position_jitter: p.position_jitter,
            scale_min: p.scale_min,
            scale_max: p.scale_max,
            random_rotation: p.random_rotation,
            min_distance_m: p.min_distance_m,
            max_slope_deg: p.max_slope_deg,
            seed: p.seed.unwrap_or(seed),
        })
    })
}

fn add_cluster(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        name: String,
        asset_palette: Vec<Weighted>,
        center: [f32; 2],
        radius_m: f32,
        count: u32,
        #[serde(default = "one")]
        scale_min: f32,
        #[serde(default = "one")]
        scale_max: f32,
        #[serde(default = "yes")]
        random_rotation: bool,
        seed: Option<u64>,
        layer: Option<String>,
    }
    let p: P = parse(params)?;
    add(project, p.layer, |id, layer, seed| {
        Generator::Cluster(Cluster {
            id,
            name: p.name,
            layer,
            asset_palette: p.asset_palette,
            center: p.center,
            radius_m: p.radius_m,
            count: p.count,
            scale_min: p.scale_min,
            scale_max: p.scale_max,
            random_rotation: p.random_rotation,
            seed: p.seed.unwrap_or(seed),
        })
    })
}

fn find<'a>(project: &'a mut Project, params: &Value) -> Result<&'a mut Generator, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    project
        .generators
        .iter_mut()
        .find(|g| g.id() == id)
        .ok_or_else(|| fail(format!("no generator `{id}`")))
}

fn brush_stroke(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        points: Vec<[f32; 2]>,
        radius_m: f32,
        #[serde(default)]
        erase: bool,
    }
    let p: P = parse(params)?;
    let Generator::MixedBrush(b) = find(project, params)? else {
        return Err(fail("only a brush takes strokes"));
    };
    b.strokes.push(Stroke {
        points: p.points,
        radius_m: p.radius_m,
        erase: p.erase,
    });
    let g = Generator::MixedBrush(b.clone());
    g.validate().map_err(|e| bad(&e))?;
    Ok(Value::Null)
}

fn update(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let g = find(project, params)?;
    let updated: Generator = merge_update(g, params)?;
    updated.validate().map_err(|e| bad(&e))?;
    let layer = updated.layer().to_string();
    *g = updated;
    check_layer(project, &layer)?;
    Ok(Value::Null)
}

fn remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = find(project, params)?.id().to_string();
    project.generators.retain(|g| g.id() != id);
    Ok(Value::Null)
}
