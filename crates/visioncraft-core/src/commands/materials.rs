use super::{fail, merge_update, next_id};
use crate::material::{Assignment, Material, MaterialMaps, SurfaceKind, Target};
use crate::project::Project;
use crate::session::{CommandError, Session, bad, parse};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    s.register(
        "material.add",
        "Add a material: {name, kind? (standard triplanar PBR), base_color? [r,g,b,a linear], metallic? (0), roughness? (0.5), maps?}; returns {id}",
        add,
    );
    s.register(
        "material.update",
        "Change fields of a material: {id, name?, kind?, base_color?, metallic?, roughness?, maps?}",
        update,
    );
    s.register(
        "material.remove",
        "Remove a material nothing is assigned: {id}",
        remove,
    );
    s.register(
        "material.assign",
        "Assign a material to a face, body or whole owner (model or shape), replacing the one there: {target: {type: face|body|owner, owner, face?, body?}, material}",
        assign,
    );
    s.register(
        "material.unassign",
        "Clear the assignment of a target: {target}",
        unassign,
    );
}

fn add(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        name: String,
        #[serde(default)]
        kind: SurfaceKind,
        #[serde(default = "white")]
        base_color: [f32; 4],
        #[serde(default)]
        metallic: f32,
        #[serde(default = "half")]
        roughness: f32,
        #[serde(default)]
        maps: MaterialMaps,
    }
    fn white() -> [f32; 4] {
        [1.0; 4]
    }
    fn half() -> f32 {
        0.5
    }
    let p: P = parse(params)?;
    let id = next_id("mat", |id| project.materials.iter().any(|m| m.id == id));
    let m = Material {
        id: id.clone(),
        name: p.name,
        kind: p.kind,
        base_color: p.base_color,
        metallic: p.metallic,
        roughness: p.roughness,
        maps: p.maps,
        extensions: Default::default(),
    };
    m.validate().map_err(|e| bad(&e))?;
    project.materials.push(m);
    Ok(json!({ "id": id }))
}

fn update(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    let at = project
        .materials
        .iter()
        .position(|m| m.id == id)
        .ok_or_else(|| fail(format!("no material `{id}`")))?;
    let m = merge_update(&project.materials[at], params)?;
    m.validate().map_err(|e| bad(&e))?;
    project.materials[at] = m;
    Ok(Value::Null)
}

fn remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    if !project.materials.iter().any(|m| m.id == id) {
        return Err(fail(format!("no material `{id}`")));
    }
    if project.assignments.iter().any(|a| a.material == id) {
        return Err(fail(format!("material `{id}` is still assigned")));
    }
    project.materials.retain(|m| m.id != id);
    Ok(Value::Null)
}

fn assign(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        target: Target,
        material: String,
    }
    let p: P = parse(params)?;
    if !project.materials.iter().any(|m| m.id == p.material) {
        return Err(fail(format!("no material `{}`", p.material)));
    }
    let owner = p.target.owner();
    if !project.models.iter().any(|m| m.id == owner)
        && !project.shapes.iter().any(|s| s.id() == owner)
    {
        return Err(fail(format!("no model or shape `{owner}`")));
    }
    // The face or body is not checked: after a re-import it may not exist for a while.
    project.assignments.retain(|a| a.target != p.target);
    project.assignments.push(Assignment {
        target: p.target,
        material: p.material,
    });
    Ok(Value::Null)
}

fn unassign(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        target: Target,
    }
    let p: P = parse(params)?;
    let before = project.assignments.len();
    project.assignments.retain(|a| a.target != p.target);
    if project.assignments.len() == before {
        return Err(fail("nothing is assigned to that target"));
    }
    Ok(Value::Null)
}
