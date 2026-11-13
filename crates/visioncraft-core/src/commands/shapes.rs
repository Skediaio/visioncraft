use super::{check_layer, fail, next_id};
use crate::material::{Assignment, Target};
use crate::project::{LAYER_SHAPES, Project};
use crate::session::{CommandError, Session, bad, parse};
use crate::shape::{Area, Extrusion, Shape, shape_mesh};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    s.register(
        "shape.add_area",
        "Add a flat area: {name, outline: [[x,z]..], elevation? (0), layer?, material? (id: also assigns it to the whole shape)}; returns {id}",
        add_area,
    );
    s.register(
        "shape.add_extrusion",
        "Add an extruded outline: {name, outline, closed? (true), base? (0), height, thickness? (0.2, open only), layer?, material?}; returns {id}",
        add_extrusion,
    );
    s.register(
        "shape.set_outline",
        "Replace the outline: {id, outline} (side materials are not remapped)",
        set_outline,
    );
    s.register(
        "shape.move_vertex",
        "Move one outline point: {id, index, position: [x,z]}",
        move_vertex,
    );
    s.register(
        "shape.insert_vertex",
        "Split edge `edge` with a new point at `position` [x,z]; the new point gets index edge+1 and later sides renumber, materials follow: {id, edge, position}",
        insert_vertex,
    );
    s.register(
        "shape.remove_vertex",
        "Remove one outline point; its two edges merge, materials follow: {id, index}",
        remove_vertex,
    );
    s.register(
        "shape.set_height",
        "Change heights: {id, base?, height?, thickness? (extrusions), elevation? (areas)}",
        set_height,
    );
    s.register(
        "shape.remove",
        "Remove a shape and its material assignments: {id}",
        remove,
    );
    s.register("shape.rename", "Rename a shape: {id, name}", rename);
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
    struct P {
        name: String,
        outline: Vec<[f32; 2]>,
        #[serde(default)]
        elevation: f32,
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
        })
    })
}

fn add_extrusion(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
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
    }
    fn yes() -> bool {
        true
    }
    fn thin() -> f32 {
        0.2
    }
    let p: P = parse(params)?;
    add(project, (p.layer, p.material), |id, layer| {
        Shape::Extrusion(Extrusion {
            id,
            name: p.name,
            layer,
            outline: p.outline,
            closed: p.closed,
            base: p.base,
            height: p.height,
            thickness: p.thickness,
        })
    })
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
    struct P {
        id: String,
        base: Option<f32>,
        height: Option<f32>,
        thickness: Option<f32>,
        elevation: Option<f32>,
    }
    let p: P = parse(params)?;
    edit(project, &p.id, |s| {
        match s {
            Shape::Area(a) => {
                if p.base.or(p.height).or(p.thickness).is_some() {
                    return Err(bad("an area has only an elevation"));
                }
                a.elevation = p.elevation.unwrap_or(a.elevation);
            }
            Shape::Extrusion(e) => {
                if p.elevation.is_some() {
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
