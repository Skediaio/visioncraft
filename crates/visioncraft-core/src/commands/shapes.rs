use super::{check_layer, fail, next_id};
use crate::material::{Assignment, Target};
use crate::project::{LAYER_SHAPES, Project};
use crate::session::{CommandError, Session, bad, parse};
use crate::shape::{Area, Extrusion, Shape, shape_mesh};
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    let layer = || param("layer", T::String, "Layer id.").default(json!("shapes"));
    let material = || {
        param(
            "material",
            T::String,
            "Material id: also assigned to the whole new shape, in the same undo step.",
        )
    };
    let outline = |doc: &str| param("outline", T::Outline, doc).required();
    s.register_with_spec(
        spec(
            "shape.add_area",
            "Add Area",
            "Draws a flat polygon (lawn, terrace, path, pond, bed) at a height; fails if the outline is self-intersecting or has fewer than 3 points.",
            vec![
                param("name", T::String, "Display name.").required(),
                outline("Polygon as [x, z] pairs in metres, either winding, not self-intersecting."),
                param("elevation", T::Number, "Height of the surface (above the ground when `drape`).").unit("m").default(json!(0)),
                param("drape", T::Boolean, "Lie on the ground: the surface follows the terrain, `elevation` is the height above it. False: flat at `elevation` (a raised deck, a pond's water).").default(json!(true)),
                layer(),
                material(),
            ],
            "{id} (S1, S2...; face names are `<id>:top`)",
            json!({"name": "Lawn", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]], "elevation": 0.0, "material": "lawn"}),
        ),
        add_area,
    );
    s.register_with_spec(
        spec(
            "shape.add_extrusion",
            "Add Extrusion",
            "Extrudes a closed polygon upwards (house volume, block), or an open polyline given a thickness (wall, fence, hedge block).",
            vec![
                param("name", T::String, "Display name.").required(),
                outline("Polygon (closed) or polyline (open) as [x, z] pairs in metres; at least 3 points closed, 2 open; not self-intersecting."),
                param("closed", T::Boolean, "True: the last point joins the first. False: a polyline widened by `thickness`.").default(json!(true)),
                param("base", T::Number, "Height of the bottom.").unit("m").default(json!(0)),
                param("height", T::Number, "Height of the solid above `base`, above zero.").unit("m").required(),
                param("thickness", T::Number, "Width of an open extrusion; ignored when closed.").unit("m").default(json!(0.2)),
                layer(),
                material(),
                param("roof", T::Object, "A closed block's roof, making it a house: {kind: flat|gable|hip|lean_to, pitch_deg, overhang_m, parapet_m}; windows and doors then go in with shape.add_opening."),
            ],
            "{id}; faces `<id>:top`, `:bottom`, `:side(n)` (n = outline edge), and `:start`, `:end` when open",
            json!({"name": "House", "outline": [[2, 2], [8, 2], [8, 6], [2, 6]], "height": 3.0, "base": 0.0}),
        ),
        add_extrusion,
    );
    s.register_with_spec(
        spec(
            "shape.set_outline",
            "Set Outline",
            "Replaces the whole outline of a shape; side materials are not renumbered (use insert/remove vertex for that).",
            vec![
                param("id", T::String, "Shape id.").required(),
                outline("New outline as [x, z] pairs in metres."),
            ],
            "null",
            json!({"id": "S2", "outline": [[2, 2], [9, 2], [9, 6], [2, 6]]}),
        ),
        set_outline,
    );
    s.register_with_spec(
        spec(
            "shape.move_vertex",
            "Move Vertex",
            "Moves one outline point; every face name stays.",
            vec![
                param("id", T::String, "Shape id.").required(),
                param(
                    "index",
                    T::Integer,
                    "Index of the point in the outline, from 0.",
                )
                .required(),
                param("position", T::Point2, "New [x, z].")
                    .unit("m")
                    .required(),
            ],
            "null",
            json!({"id": "S2", "index": 2, "position": [9, 7]}),
        ),
        move_vertex,
    );
    s.register_with_spec(
        spec(
            "shape.insert_vertex",
            "Insert Vertex",
            "Splits an edge with a new point; the new point gets index edge+1, later sides renumber and their materials follow, both halves of the split edge keep its material.",
            vec![
                param("id", T::String, "Shape id.").required(),
                param("edge", T::Integer, "Index of the edge to split (edge n runs from point n to point n+1).").required(),
                param("position", T::Point2, "The new point [x, z].").unit("m").required(),
            ],
            "null",
            json!({"id": "S2", "edge": 0, "position": [5, 1.5]}),
        ),
        insert_vertex,
    );
    s.register_with_spec(
        spec(
            "shape.remove_vertex",
            "Remove Vertex",
            "Removes an outline point; its two edges merge into the earlier one (which keeps its material); refused if the shape would become invalid.",
            vec![
                param("id", T::String, "Shape id.").required(),
                param("index", T::Integer, "Index of the point, from 0.").required(),
            ],
            "null",
            json!({"id": "S2", "index": 3}),
        ),
        remove_vertex,
    );
    s.register_with_spec(
        spec(
            "shape.set_height",
            "Set Height",
            "Changes the heights of a shape: elevation for an area; base, height and thickness for an extrusion.",
            vec![
                param("id", T::String, "Shape id.").required(),
                param("base", T::Number, "Extrusion: height of the bottom.").unit("m"),
                param("height", T::Number, "Extrusion: height above `base`, above zero.").unit("m"),
                param("thickness", T::Number, "Open extrusion: width.").unit("m"),
                param("elevation", T::Number, "Area: height of the surface.").unit("m"),
                param("drape", T::Boolean, "Area: lie on the ground (follow the terrain, `elevation` above it) or be flat."),
            ],
            "null",
            json!({"id": "S2", "height": 4.5}),
        ),
        set_height,
    );
    s.register_with_spec(
        spec(
            "shape.drape_areas",
            "Lay Areas on the Ground",
            "Areas at height 0 that are flat start to lie on the ground (follow the terrain); what terrain.create does, for projects whose areas predate their terrain.",
            vec![],
            "{draped: integer}",
            json!({}),
        ),
        |p, _| Ok(json!({"draped": drape_areas(p)})),
    );
    s.register_with_spec(
        spec(
            "shape.remove",
            "Remove Shape",
            "Removes a shape and the material assignments made to it.",
            vec![param("id", T::String, "Shape id.").required()],
            "null",
            json!({"id": "S3"}),
        ),
        remove,
    );
    s.register_with_spec(
        spec(
            "shape.rename",
            "Rename Shape",
            "Renames a shape.",
            vec![
                param("id", T::String, "Shape id.").required(),
                param("name", T::String, "New display name.").required(),
            ],
            "null",
            json!({"id": "S1", "name": "Front lawn"}),
        ),
        rename,
    );
}

fn add(
    project: &mut Project,
    (layer, material): (Option<String>, Option<String>),
    make: impl FnOnce(String, String) -> Shape,
) -> Result<Value, CommandError> {
    let layer = layer.unwrap_or_else(|| LAYER_SHAPES.into());
    check_layer(project, &layer)?;
    if let Some(m) = &material
        && project.materials.iter().all(|x| &x.id != m)
    {
        return Err(fail(format!("no material `{m}`")));
    }
    let id = next_id("S", |id| project.shapes.iter().any(|s| s.id() == id));
    let shape = make(id.clone(), layer);
    shape_mesh(&shape).map_err(|e| fail(e.to_string()))?;
    project.shapes.push(shape);
    if let Some(material) = material {
        project.assignments.push(Assignment {
            target: Target::Owner { owner: id.clone() },
            material,
        });
    }
    Ok(json!({ "id": id }))
}

fn add_area(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: String,
        outline: Vec<[f32; 2]>,
        #[serde(default)]
        elevation: f32,
        #[serde(default = "yes")]
        drape: bool,
        layer: Option<String>,
        material: Option<String>,
    }
    let p: P = parse(params)?;
    add(project, (p.layer, p.material), |id, layer| {
        Shape::Area(Area {
            id,
            name: p.name,
            layer,
            outline: p.outline,
            elevation: p.elevation,
            drape: p.drape,
            extensions: Default::default(),
        })
    })
}

fn add_extrusion(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: String,
        outline: Vec<[f32; 2]>,
        #[serde(default = "yes")]
        closed: bool,
        #[serde(default)]
        base: f32,
        height: f32,
        #[serde(default = "thin")]
        thickness: f32,
        layer: Option<String>,
        material: Option<String>,
        /// A closed block's roof (`crate::house::Roof`): it becomes a house.
        roof: Option<crate::house::Roof>,
    }
    fn yes() -> bool {
        true
    }
    fn thin() -> f32 {
        0.2
    }
    let p: P = parse(params)?;
    let house = match (p.roof, p.closed) {
        (Some(roof), true) => Some(crate::house::House {
            roof,
            ..Default::default()
        }),
        (Some(_), false) => return Err(fail("only a closed block takes a roof")),
        (None, _) => None,
    };
    let roofed = house.is_some();
    let r = add(project, (p.layer, p.material), |id, layer| {
        Shape::Extrusion(Extrusion {
            id,
            name: p.name,
            layer,
            outline: p.outline,
            closed: p.closed,
            base: p.base,
            height: p.height,
            thickness: p.thickness,
            extensions: Default::default(),
            house,
        })
    })?;
    if roofed && let Some(id) = r["id"].as_str() {
        super::house::default_materials(project, id);
    }
    Ok(r)
}

/// Applies `f` to a copy of the shape and keeps it only if it still meshes.
fn edit(
    project: &mut Project,
    id: &str,
    f: impl FnOnce(&mut Shape) -> Result<(), CommandError>,
) -> Result<(), CommandError> {
    let at = project
        .shapes
        .iter()
        .position(|s| s.id() == id)
        .ok_or_else(|| fail(format!("no shape `{id}`")))?;
    let mut shape = project.shapes[at].clone();
    f(&mut shape)?;
    shape_mesh(&shape).map_err(|e| fail(e.to_string()))?;
    project.shapes[at] = shape;
    Ok(())
}

fn set_outline(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        outline: Vec<[f32; 2]>,
    }
    let p: P = parse(params)?;
    edit(project, &p.id, |s| {
        *s.outline_mut() = p.outline;
        Ok(())
    })?;
    Ok(Value::Null)
}

fn move_vertex(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        index: usize,
        position: [f32; 2],
    }
    let p: P = parse(params)?;
    edit(project, &p.id, |s| {
        *s.outline_mut()
            .get_mut(p.index)
            .ok_or_else(|| bad("no such vertex"))? = p.position;
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Renumbers the `side(n)` assignments of shape `id`: `f(n)` gives the new indices of side `n`
/// (none drops the assignment, several copy it).
fn remap_sides(project: &mut Project, id: &str, f: impl Fn(usize) -> Vec<usize>) {
    let prefix = format!("{id}:side(");
    let old = std::mem::take(&mut project.assignments);
    for a in old {
        let n = match &a.target {
            Target::Face { owner, face } if owner == id => face
                .strip_prefix(&prefix)
                .and_then(|r| r.strip_suffix(')'))
                .and_then(|n| n.parse::<usize>().ok()),
            _ => None,
        };
        let Some(n) = n else {
            project.assignments.push(a);
            continue;
        };
        for m in f(n) {
            let mut moved = a.clone();
            moved.target = Target::Face {
                owner: id.into(),
                face: format!("{prefix}{m})"),
            };
            project.assignments.push(moved);
        }
    }
}

fn insert_vertex(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        edge: usize,
        position: [f32; 2],
    }
    let p: P = parse(params)?;
    edit(project, &p.id, |s| {
        let edges = if s.is_closed() {
            s.outline().len()
        } else {
            s.outline().len().saturating_sub(1)
        };
        if p.edge >= edges {
            return Err(bad("no such edge"));
        }
        s.outline_mut().insert(p.edge + 1, p.position);
        Ok(())
    })?;
    // The split edge keeps its material on both halves.
    remap_sides(project, &p.id, |n| match n.cmp(&p.edge) {
        std::cmp::Ordering::Less => vec![n],
        std::cmp::Ordering::Equal => vec![n, n + 1],
        std::cmp::Ordering::Greater => vec![n + 1],
    });
    Ok(Value::Null)
}

fn remove_vertex(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        index: usize,
    }
    let p: P = parse(params)?;
    let mut last_of_open = false;
    edit(project, &p.id, |s| {
        let len = s.outline().len();
        if p.index >= len {
            return Err(bad("no such vertex"));
        }
        last_of_open = !s.is_closed() && p.index == len - 1;
        s.outline_mut().remove(p.index);
        Ok(())
    })?;
    // Edges `index-1` and `index` merge into one that keeps the number `index-1` and its material;
    // the material of edge `index` goes. Removing the end of an open line drops its one edge.
    let dropped = if last_of_open { p.index - 1 } else { p.index };
    remap_sides(project, &p.id, |n| match n.cmp(&dropped) {
        std::cmp::Ordering::Less => vec![n],
        std::cmp::Ordering::Equal => vec![],
        std::cmp::Ordering::Greater => vec![n - 1],
    });
    Ok(Value::Null)
}

fn set_height(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        base: Option<f32>,
        height: Option<f32>,
        thickness: Option<f32>,
        elevation: Option<f32>,
        drape: Option<bool>,
    }
    let p: P = parse(params)?;
    edit(project, &p.id, |s| {
        match s {
            Shape::Area(a) => {
                if p.base.or(p.height).or(p.thickness).is_some() {
                    return Err(bad("an area has only an elevation"));
                }
                a.elevation = p.elevation.unwrap_or(a.elevation);
                a.drape = p.drape.unwrap_or(a.drape);
            }
            Shape::Extrusion(e) => {
                if p.elevation.is_some() || p.drape.is_some() {
                    return Err(bad("an extrusion has base, height and thickness"));
                }
                e.base = p.base.unwrap_or(e.base);
                e.height = p.height.unwrap_or(e.height);
                e.thickness = p.thickness.unwrap_or(e.thickness);
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    let before = project.shapes.len();
    project.shapes.retain(|s| s.id() != id);
    if project.shapes.len() == before {
        return Err(fail(format!("no shape `{id}`")));
    }
    project.assignments.retain(|a| a.target.owner() != id);
    Ok(Value::Null)
}

fn rename(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        name: String,
    }
    let p: P = parse(params)?;
    let s = project
        .shapes
        .iter_mut()
        .find(|s| s.id() == p.id)
        .ok_or_else(|| fail(format!("no shape `{}`", p.id)))?;
    *s.name_mut() = p.name;
    Ok(Value::Null)
}

pub(crate) fn yes() -> bool {
    true
}

/// Areas at height 0 that are flat start to lie on the ground; returns how many.
pub(crate) fn drape_areas(project: &mut Project) -> usize {
    let mut n = 0;
    for s in &mut project.shapes {
        if let Shape::Area(a) = s
            && !a.drape
            && a.elevation == 0.0
        {
            a.drape = true;
            n += 1;
        }
    }
    n
}
