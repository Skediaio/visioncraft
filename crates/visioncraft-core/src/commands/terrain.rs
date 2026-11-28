use super::fail;
use super::shapes::{add_area, add_extrusion};
use crate::project::Project;
use crate::session::{CommandError, Session, bad, parse};
use crate::shape::Shape;
use crate::spec::{ParamType as T, param, spec};
use crate::terrain::{BrushStroke, LayerDetail, MAX_LAYERS, Terrain, TerrainError, apply_stroke};
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
            "Creates a flat terrain at height 0 in the base material; refused if one exists. At most 1024 cells per side. Areas at height 0 start to lie on it (drape).",
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
            "terrain.set_layer_detail",
            "Set Layer Detail Texture",
            "Gives a terrain layer a second, finer detail texture over its own maps: tiled once per `tile_m` metres, modulating the layer's colour (and its normal when the texture has a normal map) by `strength`, at full strength near the eye and faded out by `fade_m`, projected triplanar on steep ground so it does not stretch. A large field or desert then neither blurs up close nor repeats from afar. `texture` is a texture or material id the library resolves (a project material, or a library material's id: its colour and normal maps). Without `texture` the layer's detail is removed, and no other field may be given then. The detail keeps the layer's own brightness (the texture is taken relative to its mean). Costs up to two samples more per layer near the camera.",
            vec![
                param("layer", T::Integer, "Terrain layer index, from 0.").required(),
                param("texture", T::String, "Texture or material id of the detail (its colour and normal maps). Left out: the detail is removed."),
                param("tile_m", T::Number, "One repeat of the detail per this many metres, 0.01..1000.").unit("m").default(json!(1.0)),
                param("strength", T::Number, "0..1: how strongly the detail modulates the layer.").default(json!(0.5)),
                param("fade_m", T::Number, "Distance to the eye at which the detail has faded out (full up to 0.6 of it), 0.1..100000.").unit("m").default(json!(30.0)),
            ],
            "{layer, texture, tile_m, strength, fade_m} as set; null when removed",
            json!({"layer": 1, "texture": "gravel", "tile_m": 0.5, "strength": 0.6, "fade_m": 20}),
        ),
        set_layer_detail,
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
            "Lowers the ground inside an outline by a depth, with banks sloping up over a width outside it: a pond bed. It holds no water by itself: the result's `note` says so when no water area lies over it (terrain.add_pond carves, fills and edges a pond in one step).",
            vec![
                outline(),
                param("depth_m", T::Number, "How far to lower the ground.").unit("m").required(),
                param("bank_m", T::Number, "Width of the sloped bank, 0 or more.").unit("m").required(),
                param("bed_material", T::String, "Paints the basin and its bank with this material (a terrain layer is added for it if needed)."),
            ],
            "null or {note}",
            json!({"outline": [[5, 5], [10, 5], [10, 9], [5, 9]], "depth_m": 0.8, "bank_m": 1.5}),
        ),
        carve,
    );
    s.register_with_spec(
        spec(
            "terrain.add_pond",
            "Add Pond",
            "A pond in one step, as the Pond tool makes it: the ground carved inside the outline with a straight wall and a soil bed, a level water surface just under the lowest ground on the outline, and a stone coping round the edge (at least one ground cell wide, so it covers the wall). Needs a terrain.",
            vec![
                outline(),
                param("depth_m", T::Number, "Depth of the bed below the ground, above zero.").unit("m").default(json!(0.8)),
            ],
            "{water, edge}",
            json!({"outline": [[5, 5], [10, 5], [10, 9], [5, 9]], "depth_m": 0.8}),
        ),
        add_pond,
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
    // Areas that lay on the old flat ground lie on the new one (see `shape`'s module docs).
    super::shapes::drape_areas(project);
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

/// What `terrain.set_layer_detail` assumes for a field that is not given.
const DETAIL_TILE_M: f32 = 1.0;
const DETAIL_STRENGTH: f32 = 0.5;
const DETAIL_FADE_M: f32 = 30.0;

fn set_layer_detail(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        layer: usize,
        texture: Option<String>,
        tile_m: Option<f32>,
        strength: Option<f32>,
        fade_m: Option<f32>,
    }
    let p: P = parse(params)?;
    let t = edit(project)?;
    let count = t.layers.len();
    let layer = t.layers.get_mut(p.layer).ok_or_else(|| {
        fail(format!(
            "no terrain layer {} (the terrain has {count} layers)",
            p.layer
        ))
    })?;
    let Some(texture) = p.texture else {
        if p.tile_m.is_some() || p.strength.is_some() || p.fade_m.is_some() {
            return Err(bad(
                "without `texture` the layer's detail is removed: give no tile_m, strength or fade_m then",
            ));
        }
        layer.detail = None;
        return Ok(Value::Null);
    };
    let detail = LayerDetail {
        texture,
        tile_m: p.tile_m.unwrap_or(DETAIL_TILE_M),
        strength: p.strength.unwrap_or(DETAIL_STRENGTH),
        fade_m: p.fade_m.unwrap_or(DETAIL_FADE_M),
    };
    detail.validate().map_err(terr)?;
    layer.detail = Some(detail.clone());
    Ok(json!({
        "layer": p.layer,
        "texture": detail.texture,
        "tile_m": detail.tile_m,
        "strength": detail.strength,
        "fade_m": detail.fade_m,
    }))
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
        Shape::Area(a) => (a.outline().into_owned(), a.elevation),
        Shape::Extrusion(e) if e.closed => (e.outline().into_owned(), e.base),
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
        bed_material: Option<String>,
    }
    let p: P = parse(params)?;
    if let Some(m) = &p.bed_material {
        material_exists(project, m)?;
    }
    if p.outline.len() < 3
        || !p.outline.iter().flatten().all(|c| c.is_finite())
        || !p.depth_m.is_finite()
        || !(p.bank_m.is_finite() && p.bank_m >= 0.0)
    {
        return Err(bad(
            "an outline of 3 points, a finite depth and a bank of 0 or more",
        ));
    }
    let t = edit(project)?;
    t.carve(&p.outline, p.depth_m, p.bank_m);
    if let Some(m) = &p.bed_material {
        let layer = match t.layers.iter().position(|l| &l.material == m) {
            Some(i) => i,
            None => t
                .add_layer(m)
                .ok_or_else(|| fail("the terrain has no room for another material layer"))?,
        };
        t.paint_outline(&p.outline, p.bank_m, layer);
    }
    let water = crate::plant_ground::water_areas(project);
    let filled = project.shapes.iter().any(|s| match s {
        Shape::Area(a) => water.contains(&a.id) && crate::shape::overlaps(&a.outline(), &p.outline),
        Shape::Extrusion(_) => false,
    });
    if filled || p.depth_m <= 0.0 {
        return Ok(Value::Null);
    }
    Ok(
        json!({"note": "the basin holds no water: add a level water area over it (shape.add_area with drape false, material water, its elevation just under the ground on the outline), or use terrain.add_pond, which carves, fills and edges a pond in one step"}),
    )
}

/// The Pond tool's coping: this wide at least (and one ground cell), 12 cm over the old rim,
/// reaching 3 cm over the water.
const POND_COPING_M: f32 = 0.3;
const POND_COPING_H: f32 = 0.12;
const POND_OVERHANG_M: f32 = 0.03;
/// The water stands this far below the lowest ground on the outline, so the lawn meets the
/// water instead of a dark strip of bank.
const POND_MARGIN_M: f32 = 0.05;

fn add_pond(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        outline: Vec<[f32; 2]>,
        #[serde(default = "depth")]
        depth_m: f32,
    }
    fn depth() -> f32 {
        0.8
    }
    let p: P = parse(params)?;
    if !(p.depth_m.is_finite() && p.depth_m > 0.0) {
        return Err(bad("the depth must be above zero"));
    }
    let terrain = project
        .terrain
        .clone()
        .ok_or_else(|| fail("there is no terrain: terrain.create first"))?;
    let water = p.outline;
    let rim = water
        .iter()
        .filter_map(|q| terrain.height_at(q[0], q[1]))
        .reduce(f32::min)
        .unwrap_or(0.0);
    // Carved a centimetre wider than the water, with a straight wall: the ground's cells then
    // slope up under the coping, outside the water, on every side alike.
    carve(
        project,
        &json!({"outline": crate::shape::offset_outline(&water, 0.01), "depth_m": p.depth_m, "bank_m": 0.0, "bed_material": "soil"}),
    )?;
    let w = add_area(
        project,
        &json!({"name": "Pond", "outline": water, "elevation": rim - POND_MARGIN_M, "drape": false, "material": "water"}),
    )?;
    // An open band round the edge (a closed one would be a solid block), from the middle of the
    // first edge round to a centimetre short of it; its corners mitre.
    let width = POND_COPING_M.max(terrain.cell_m);
    let band = crate::shape::offset_outline(&water, width / 2.0 - POND_OVERHANG_M);
    let (a, b) = (band[0], band[1 % band.len()]);
    let mid = |t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2))
        .sqrt()
        .max(0.1);
    let mut ring = vec![mid(0.5)];
    ring.extend(band.iter().skip(1));
    ring.push(a);
    ring.push(mid(0.5 - 0.01 / len));
    // The band stands on the lowest ground under it (down the carved wall): as much taller that
    // its top is the same height over the old rim all round.
    let lowest = project.terrain.as_ref().map_or(rim, |t| {
        crate::shape::footing(&ring, &|x, z| t.height_at(x, z))
    });
    let e = add_extrusion(
        project,
        &json!({"name": "Pond edge", "outline": ring, "closed": false, "thickness": width,
                "height": (rim - lowest).max(0.0) + POND_COPING_H, "material": "paving"}),
    )?;
    Ok(json!({"water": w["id"], "edge": e["id"]}))
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

#[cfg(test)]
mod tests {
    use crate::Session;
    use serde_json::{Value, json};

    /// JSON values alike, numbers to within an f32 (a detail's fields are f32: 0.7 round-trips as
    /// 0.699999988).
    fn nearly(a: &serde_json::Value, b: &serde_json::Value) -> bool {
        use serde_json::Value;
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                (x.as_f64().unwrap_or(0.0) - y.as_f64().unwrap_or(0.0)).abs() < 1e-5
            }
            (Value::Object(x), Value::Object(y)) => {
                x.len() == y.len()
                    && x.iter()
                        .all(|(k, v)| y.get(k).is_some_and(|w| nearly(v, w)))
            }
            (Value::String(x), Value::String(y)) => x == y,
            (Value::Null, Value::Null) => true,
            _ => false,
        }
    }

    /// The estate's basin on its 1 m terrain: a carve alone says it holds no water; the pond
    /// command fills it level, edges it, and the lawn round it ends at the water's own edge.
    #[test]
    fn a_pond_on_a_coarse_terrain_is_filled_edged_and_cut_exactly() {
        let pond = json!([[55, 40], [65, 40], [66, 48], [54, 47]]);
        let mut s = Session::new(Default::default());
        s.run("terrain.create", json!({"origin": [-10, -10], "width_m": 120, "depth_m": 80, "cell_m": 1.0, "base_material": "lawn"}))
            .unwrap();
        s.run("shape.add_area", json!({"name": "South lawn", "outline": [[22, 30], [100, 30], [100, 60], [22, 60]], "material": "lawn"}))
            .unwrap();
        let r = s
            .run(
                "terrain.carve",
                json!({"outline": pond, "depth_m": 0.9, "bank_m": 1.5}),
            )
            .unwrap();
        assert!(r["note"].as_str().unwrap().contains("no water"), "{r}");
        assert!(s.undo());

        let r = s
            .run("terrain.add_pond", json!({"outline": pond, "depth_m": 0.9}))
            .unwrap();
        let p = s.project().clone();
        let t = p.terrain.as_deref().unwrap();
        let water = p.shapes.iter().find(|x| x.id() == r["water"]).unwrap();
        let crate::Shape::Area(w) = water else {
            panic!()
        };
        assert!(
            !w.drape && (w.elevation + 0.05).abs() < 1e-4,
            "{}",
            w.elevation
        );
        assert!(t.height_at(60.0, 44.0).unwrap() < -0.85, "the bed");
        let edge = p.shapes.iter().find(|x| x.id() == r["edge"]).unwrap();
        let crate::Shape::Extrusion(e) = edge else {
            panic!()
        };
        assert!(
            !e.closed && e.thickness >= t.cell_m,
            "the coping covers a cell"
        );

        let lawn = p.shapes.iter().find(|x| x.name() == "South lawn").unwrap();
        let lifts: std::collections::HashMap<String, f32> =
            crate::area_lifts(&p.shapes).into_iter().collect();
        let (m, _) = crate::placed_shape_mesh(&p, lawn, &lifts, &crate::water_areas(&p)).unwrap();
        let plan: f32 = m
            .triangles
            .iter()
            .map(|tri| {
                let [a, b, c] = tri.map(|i| m.positions[i as usize]);
                ((b[0] - a[0]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[0] - a[0])).abs() / 2.0
            })
            .sum();
        // 78 x 30 m less the 82.5 m2 of water, to a few square centimetres.
        assert!((plan - (2340.0 - 82.5)).abs() < 0.05, "{plan}");
        assert!(
            s.run(
                "terrain.carve",
                json!({"outline": pond, "depth_m": 0.9, "bank_m": 0})
            )
            .unwrap()
            .is_null(),
            "water lies over it now"
        );
    }

    /// `terrain.set_layer_detail` sets a layer's detail (the defaults filled in), it reads back
    /// from the project, without `texture` it clears the detail, every range error names its
    /// field, and it is one undo step each way.
    #[test]
    fn a_layer_detail_is_set_read_back_cleared_and_undone() {
        let mut s = Session::new(Default::default());
        s.run(
            "terrain.create",
            json!({"origin": [0, 0], "width_m": 10, "depth_m": 10, "base_material": "lawn"}),
        )
        .unwrap();
        s.run("terrain.add_layer", json!({"material": "gravel"}))
            .unwrap();
        let detail =
            json!({"layer": 0, "texture": "sand", "tile_m": 0.5, "strength": 0.6, "fade_m": 20});
        let read = |s: &Session| {
            serde_json::to_value(&s.project().terrain.as_deref().unwrap().layers).unwrap()[0]["detail"].clone()
        };
        assert_eq!(read(&s), Value::Null, "no detail to begin with");
        let r = s.run("terrain.set_layer_detail", detail.clone()).unwrap();
        assert!(
            nearly(
                &r,
                &json!({"layer": 0, "texture": "sand", "tile_m": 0.5, "strength": 0.6, "fade_m": 20.0})
            ),
            "{r}"
        );
        assert!(
            nearly(
                &read(&s),
                &json!({"texture": "sand", "tile_m": 0.5, "strength": 0.6, "fade_m": 20.0})
            ),
            "{}",
            read(&s)
        );
        // The defaults.
        let r = s
            .run(
                "terrain.set_layer_detail",
                json!({"layer": 0, "texture": "gravel"}),
            )
            .unwrap();
        assert!(
            nearly(
                &r,
                &json!({"layer": 0, "texture": "gravel", "tile_m": 1.0, "strength": 0.5, "fade_m": 30.0})
            ),
            "{r}"
        );
        // Setting the same again changes nothing: no undo step, no revision.
        let (rev, can_undo) = (s.revision(), s.can_undo());
        s.run(
            "terrain.set_layer_detail",
            json!({"layer": 0, "texture": "gravel"}),
        )
        .unwrap();
        assert_eq!((s.revision(), s.can_undo()), (rev, can_undo));
        // Clearing.
        assert!(
            s.run("terrain.set_layer_detail", json!({"layer": 0}))
                .unwrap()
                .is_null()
        );
        assert_eq!(read(&s), Value::Null);
        assert!(s.undo());
        assert_eq!(read(&s)["texture"], "gravel");
        assert!(s.undo());
        assert_eq!(read(&s)["texture"], "sand");
        assert!(s.redo());
        assert!(s.redo());
        assert_eq!(read(&s), Value::Null);
        // Two redos brought the cleared detail back; every range error names its field and
        // changes nothing.
        assert_eq!(read(&s), Value::Null);
        for (field, value) in [
            ("tile_m", json!(0.0)),
            ("tile_m", json!(2000.0)),
            ("strength", json!(1.01)),
            ("strength", json!(-0.2)),
            ("fade_m", json!(0.0)),
        ] {
            let mut p = detail.clone();
            p[field] = value.clone();
            let e = s
                .run("terrain.set_layer_detail", p)
                .unwrap_err()
                .to_string();
            assert!(e.contains(field), "{field}={value}: {e}");
        }
        // No such layer; no texture named; a clear takes no other field; no terrain.
        let e = s
            .run(
                "terrain.set_layer_detail",
                json!({"layer": 5, "texture": "sand"}),
            )
            .unwrap_err()
            .to_string();
        assert!(e.contains("layer 5") && e.contains("2 layers"), "{e}");
        let e = s
            .run(
                "terrain.set_layer_detail",
                json!({"layer": 0, "texture": ""}),
            )
            .unwrap_err()
            .to_string();
        assert!(e.contains("texture"), "{e}");
        let e = s
            .run(
                "terrain.set_layer_detail",
                json!({"layer": 0, "strength": 0.5}),
            )
            .unwrap_err()
            .to_string();
        assert!(e.contains("texture") && e.contains("strength"), "{e}");
        s.run("terrain.remove", json!({})).unwrap();
        let e = s
            .run(
                "terrain.set_layer_detail",
                json!({"layer": 0, "texture": "sand"}),
            )
            .unwrap_err()
            .to_string();
        assert!(e.contains("no terrain"), "{e}");
    }
}
