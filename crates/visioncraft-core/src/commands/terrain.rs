use super::fail;
use crate::project::Project;
use crate::session::{CommandError, Session, bad, parse};
use crate::shape::Shape;
use crate::spec::{ParamType as T, param, spec};
use crate::terrain::{BrushStroke, MAX_LAYERS, Terrain, TerrainError, apply_stroke};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) fn register(s: &mut Session) {
    let outline = || {
        param(
            "outline",
            T::Outline,
            "Polygon as [x, z] pairs in metres, at least 3.",
        )
        .required()
    };
    s.register_with_spec(
        spec(
            "terrain.create",
            "Create Terrain",
            "Creates a flat terrain at height 0 in the base material; refused if one exists. At most 1024 cells per side.",
            vec![
                param("origin", T::Point2, "[x, z] of the corner with the smallest coordinates.").unit("m").required(),
                param("width_m", T::Number, "Size along x, above zero.").unit("m").required(),
                param("depth_m", T::Number, "Size along z, above zero.").unit("m").required(),
                param("cell_m", T::Number, "Grid cell size, above zero.").unit("m").default(json!(0.5)),
                param("base_material", T::String, "Material id of layer 0.").required(),
            ],
            "null",
            json!({"origin": [0, 0], "width_m": 40, "depth_m": 30, "base_material": "lawn"}),
        ),
        create,
    );
    s.register_with_spec(
        spec(
            "terrain.resize",
            "Resize Terrain",
            "Changes the terrain's origin and size, keeping height and paint where the old grid overlaps; the cell size stays.",
            vec![
                param("origin", T::Point2, "New corner [x, z].").unit("m").required(),
                param("width_m", T::Number, "New size along x.").unit("m").required(),
                param("depth_m", T::Number, "New size along z.").unit("m").required(),
            ],
            "null",
            json!({"origin": [-5, -5], "width_m": 50, "depth_m": 40}),
        ),
        resize,
    );
    s.register_with_spec(
        spec(
            "terrain.remove",
            "Remove Terrain",
            "Removes the terrain.",
            vec![],
            "null",
            json!({}),
        ),
        remove,
    );
    s.register_with_spec(
        spec(
            "terrain.add_layer",
            "Add Terrain Layer",
            "Adds a material layer (at most 8) with no paint yet.",
            vec![param("material", T::String, "Material id.").required()],
            "{index}",
            json!({"material": "soil"}),
        ),
        add_layer,
    );
    s.register_with_spec(
        spec(
            "terrain.remove_layer",
            "Remove Terrain Layer",
            "Removes a layer and shares its paint among the others; the last layer cannot go.",
            vec![param("index", T::Integer, "Layer index, from 0.").required()],
            "null",
            json!({"index": 1}),
        ),
        remove_layer,
    );
    s.register_with_spec(
        spec(
            "terrain.set_layer_material",
            "Set Layer Material",
            "Changes the material of a terrain layer.",
            vec![
                param("index", T::Integer, "Layer index, from 0.").required(),
                param("material", T::String, "Material id.").required(),
            ],
            "null",
            json!({"index": 1, "material": "soil"}),
        ),
        set_layer_material,
    );
    s.register_with_spec(
        spec(
            "terrain.stroke",
            "Terrain Brush Stroke",
            "Applies a whole brush stroke along a path as one undo step. Dabs sit a quarter radius apart, so strength accumulates; use small values.",
            vec![
                param("tool", T::String, "raise/lower: metres per dab; flatten: towards `target_height` or the height under the first point; smooth; noise: deterministic value noise; paint: moves paint towards `layer`; ramp: slope between the first and last point across the brush width.")
                    .required()
                    .allowed(&["raise", "lower", "flatten", "smooth", "noise", "paint", "ramp"]),
                param("points", T::Outline, "Path as [x, z] pairs in metres, at least 1.").required(),
                param("radius_m", T::Number, "Brush radius, above zero.").unit("m").required(),
                param("strength", T::Number, "Raise, lower, noise: metres per dab at full weight. Others: fraction 0..1 moved towards the goal per dab.").required(),
                param("falloff", T::Number, "0..1: full strength inside radius*(1-falloff), smoothstep to the edge.").default(json!(0.5)),
                param("target_height", T::Number, "Flatten: the height to reach.").unit("m"),
                param("layer", T::Integer, "Paint: the terrain layer index to paint."),
            ],
            "null",
            json!({"tool": "raise", "points": [[10, 10], [20, 12]], "radius_m": 4, "strength": 0.1}),
        ),
        stroke,
    );
    s.register_with_spec(
        spec(
            "terrain.level_under",
            "Level Ground Under Shape",
            "Sets the ground under an area (its elevation) or a closed extrusion (its base) to that height, blending to the old ground over a margin outside it.",
            vec![
                param("shape", T::String, "Shape id.").required(),
                param("margin_m", T::Number, "Width of the smooth skirt outside the outline; 0 for none.").unit("m").required(),
            ],
            "null",
            json!({"shape": "S1", "margin_m": 1.5}),
        ),
        level_under,
    );
    s.register_with_spec(
        spec(
            "terrain.carve",
            "Carve Terrain",
            "Lowers the ground inside an outline by a depth, with banks sloping up over a width outside it: a pond bed.",
            vec![
                outline(),
                param("depth_m", T::Number, "How far to lower the ground.").unit("m").required(),
                param("bank_m", T::Number, "Width of the sloped bank, 0 or more.").unit("m").required(),
            ],
            "null",
            json!({"outline": [[5, 5], [10, 5], [10, 9], [5, 9]], "depth_m": 0.8, "bank_m": 1.5}),
        ),
        carve,
    );
    s.register_with_spec(
        spec(
            "terrain.import_heights",
            "Import Heights",
            "Replaces the grid with the given heights (for elevation data); paint resets to the base material.",
            vec![
                param("origin", T::Point2, "Corner [x, z].").unit("m").required(),
                param("width", T::Integer, "Cells along x; there are width+1 vertices.").required(),
                param("depth", T::Integer, "Cells along z.").required(),
                param("cell_m", T::Number, "Cell size.").unit("m").required(),
                param("heights", T::Array, "Metres, row-major (x fastest), (width+1)*(depth+1) numbers.").required(),
                param("base_material", T::String, "Material id of layer 0; needed when there is no terrain yet, else the current base."),
            ],
            "null",
            json!({"origin": [0, 0], "width": 2, "depth": 2, "cell_m": 1.0, "heights": [0, 0, 0, 0, 1, 0, 0, 0, 0]}),
        ),
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
    #[serde(deny_unknown_fields)]
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
    #[serde(deny_unknown_fields)]
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
    #[serde(deny_unknown_fields)]
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
    #[serde(deny_unknown_fields)]
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
    #[serde(deny_unknown_fields)]
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
    #[serde(deny_unknown_fields)]
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
