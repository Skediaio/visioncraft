use super::fail;
use crate::project::Project;
use crate::session::{CommandError, Session, bad, parse};
use crate::shape::Shape;
use crate::terrain::{BrushStroke, MAX_LAYERS, Terrain, TerrainError, apply_stroke};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub(super) fn register(s: &mut Session) {
    s.register(
        "terrain.create",
        "Create a flat terrain: {origin [x,z], width_m, depth_m, cell_m? (0.5), base_material}; refused if one exists",
        create,
    );
    s.register(
        "terrain.resize",
        "Change origin and size, keeping data where it overlaps: {origin [x,z], width_m, depth_m}",
        resize,
    );
    s.register("terrain.remove", "Remove the terrain", remove);
    s.register(
        "terrain.add_layer",
        "Add a material layer (up to 8): {material}; returns {index}",
        add_layer,
    );
    s.register(
        "terrain.remove_layer",
        "Remove a layer, sharing its weight among the others: {index}",
        remove_layer,
    );
    s.register(
        "terrain.set_layer_material",
        "Change a layer's material: {index, material}",
        set_layer_material,
    );
    s.register(
        "terrain.stroke",
        "One brush stroke, one undo step: {tool: raise|lower|flatten|smooth|noise|paint|ramp, points: [[x,z]..], radius_m, strength, falloff? (0.5), target_height?, layer?}",
        stroke,
    );
    s.register(
        "terrain.level_under",
        "Level the ground under an area or closed extrusion to its elevation or base, with a smooth skirt: {shape, margin_m}",
        level_under,
    );
    s.register(
        "terrain.carve",
        "Lower the ground inside an outline with sloped banks: {outline [[x,z]..], depth_m, bank_m}",
        carve,
    );
    s.register(
        "terrain.import_heights",
        "Replace the grid with given heights (row-major, (width+1)*(depth+1) values): {origin, width, depth, cell_m, heights, base_material?}",
        import_heights,
    );
}

fn terr(e: TerrainError) -> CommandError {
    fail(e.0)
}

fn material_exists(project: &Project, id: &str) -> Result<(), CommandError> {
    if project.materials.iter().any(|m| m.id == id) {
        Ok(())
    } else {
        Err(fail(format!("no material `{id}`")))
    }
}

/// The terrain to edit: copied once if an undo snapshot still shares it.
fn edit(project: &mut Project) -> Result<&mut Terrain, CommandError> {
    project
        .terrain
        .as_mut()
        .map(Arc::make_mut)
        .ok_or_else(|| fail("there is no terrain"))
}

fn create(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        origin: [f32; 2],
        width_m: f32,
        depth_m: f32,
        #[serde(default = "cell")]
        cell_m: f32,
        base_material: String,
    }
    fn cell() -> f32 {
        0.5
    }
    let p: P = parse(params)?;
    if project.terrain.is_some() {
        return Err(fail("a terrain exists already"));
    }
    material_exists(project, &p.base_material)?;
    project.terrain = Some(Arc::new(
        Terrain::flat(p.origin, p.width_m, p.depth_m, p.cell_m, &p.base_material).map_err(terr)?,
    ));
    Ok(Value::Null)
}

fn resize(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        origin: [f32; 2],
        width_m: f32,
        depth_m: f32,
    }
    let p: P = parse(params)?;
    let t = edit(project)?;
    *t = t.resized(p.origin, p.width_m, p.depth_m).map_err(terr)?;
    Ok(Value::Null)
}

fn remove(project: &mut Project, _: &Value) -> Result<Value, CommandError> {
    edit(project)?;
    project.terrain = None;
    Ok(Value::Null)
}

fn add_layer(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let material = params["material"]
        .as_str()
        .ok_or_else(|| bad("missing material"))?;
    material_exists(project, material)?;
    let index = edit(project)?
        .add_layer(material)
        .ok_or_else(|| fail(format!("a terrain has at most {MAX_LAYERS} layers")))?;
    Ok(serde_json::json!({ "index": index }))
}

fn remove_layer(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let index = params["index"]
        .as_u64()
        .ok_or_else(|| bad("missing index"))? as usize;
    edit(project)?.remove_layer(index).map_err(terr)?;
    Ok(Value::Null)
}

fn set_layer_material(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        index: usize,
        material: String,
    }
    let p: P = parse(params)?;
    material_exists(project, &p.material)?;
    let layer = edit(project)?
        .layers
        .get_mut(p.index)
        .ok_or_else(|| bad("no such layer"))?;
    layer.material = p.material;
    Ok(Value::Null)
}

fn stroke(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        #[serde(flatten)]
        stroke: BrushStroke,
    }
    // `falloff` is optional here.
    let mut value = params.clone();
    if let Some(o) = value.as_object_mut() {
        o.entry("falloff").or_insert(0.5.into());
    }
    let p: P = parse(&value)?;
    let t = project
        .terrain
        .as_ref()
        .ok_or_else(|| fail("there is no terrain"))?;
    p.stroke.validate(t).map_err(terr)?;
    apply_stroke(edit(project)?, &p.stroke);
    Ok(Value::Null)
}

fn level_under(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        shape: String,
        margin_m: f32,
    }
    let p: P = parse(params)?;
    if !(p.margin_m.is_finite() && p.margin_m >= 0.0) {
        return Err(bad("margin must be 0 or more"));
    }
    let shape = project
        .shapes
        .iter()
        .find(|s| s.id() == p.shape)
        .ok_or_else(|| fail(format!("no shape `{}`", p.shape)))?;
    let (outline, height) = match shape {
        Shape::Area(a) => (a.outline.clone(), a.elevation),
        Shape::Extrusion(e) if e.closed => (e.outline.clone(), e.base),
        Shape::Extrusion(_) => {
            return Err(fail("only an area or a closed extrusion levels the ground"));
        }
    };
    edit(project)?.level_under(&outline, height, p.margin_m);
    Ok(Value::Null)
}

fn carve(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        outline: Vec<[f32; 2]>,
        depth_m: f32,
        bank_m: f32,
    }
    let p: P = parse(params)?;
    if p.outline.len() < 3
        || !p.outline.iter().flatten().all(|c| c.is_finite())
        || !p.depth_m.is_finite()
        || !(p.bank_m.is_finite() && p.bank_m >= 0.0)
    {
        return Err(bad(
            "an outline of 3 points, a finite depth and a bank of 0 or more",
        ));
    }
    edit(project)?.carve(&p.outline, p.depth_m, p.bank_m);
    Ok(Value::Null)
}

fn import_heights(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        origin: [f32; 2],
        width: u32,
        depth: u32,
        cell_m: f32,
        heights: Vec<f32>,
        base_material: Option<String>,
    }
    let p: P = parse(params)?;
    let base = match (&p.base_material, &project.terrain) {
        (Some(m), _) => m.clone(),
        (None, Some(t)) => t.layers[0].material.clone(),
        (None, None) => return Err(bad("base_material is needed when there is no terrain yet")),
    };
    material_exists(project, &base)?;
    let mut t = Terrain::flat(
        p.origin,
        p.width as f32 * p.cell_m,
        p.depth as f32 * p.cell_m,
        p.cell_m,
        &base,
    )
    .map_err(terr)?;
    if (t.width, t.depth) != (p.width, p.depth) || t.heights.len() != p.heights.len() {
        return Err(bad("heights must hold (width+1)*(depth+1) values"));
    }
    t.heights = p.heights;
    t.validate().map_err(terr)?;
    project.terrain = Some(Arc::new(t));
    Ok(Value::Null)
}
