use super::{fail, merge_update, next_id};
use crate::project::{Project, SunTime};
use crate::session::{CommandError, Session, bad, parse};
use crate::viewpoint::Viewpoint;
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    s.register(
        "viewpoint.add",
        "Save a camera: {name, position, target, fov_y_deg? (60), time?}; returns {id}",
        viewpoint_add,
    );
    s.register(
        "viewpoint.update",
        "Change a viewpoint: {id, name?, position?, target?, fov_y_deg?, time? (null clears)}",
        viewpoint_update,
    );
    s.register(
        "viewpoint.remove",
        "Remove a viewpoint: {id}",
        viewpoint_remove,
    );
    s.register(
        "viewpoint.reorder",
        "Move a viewpoint to a place in the list: {id, index}",
        viewpoint_reorder,
    );
}

fn at(project: &Project, id: &str) -> Result<usize, CommandError> {
    project
        .viewpoints
        .iter()
        .position(|v| v.id == id)
        .ok_or_else(|| fail(format!("no viewpoint `{id}`")))
}

fn viewpoint_add(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        name: String,
        position: [f32; 3],
        target: [f32; 3],
        #[serde(default = "fov")]
        fov_y_deg: f32,
        time: Option<SunTime>,
    }
    fn fov() -> f32 {
        60.0
    }
    let p: P = parse(params)?;
    let id = next_id("v", |id| project.viewpoints.iter().any(|v| v.id == id));
    let v = Viewpoint {
        id: id.clone(),
        name: p.name,
        position: p.position,
        target: p.target,
        fov_y_deg: p.fov_y_deg,
        time: p.time,
    };
    v.validate().map_err(|e| bad(&e))?;
    project.viewpoints.push(v);
    Ok(json!({ "id": id }))
}

fn viewpoint_update(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let i = at(
        project,
        params["id"].as_str().ok_or_else(|| bad("missing id"))?,
    )?;
    let v = merge_update(&project.viewpoints[i], params)?;
    v.validate().map_err(|e| bad(&e))?;
    project.viewpoints[i] = v;
    Ok(Value::Null)
}

fn viewpoint_remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let i = at(
        project,
        params["id"].as_str().ok_or_else(|| bad("missing id"))?,
    )?;
    project.viewpoints.remove(i);
    Ok(Value::Null)
}

fn viewpoint_reorder(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        id: String,
        index: usize,
    }
    let p: P = parse(params)?;
    let i = at(project, &p.id)?;
    if p.index >= project.viewpoints.len() {
        return Err(bad("index is past the end of the list"));
    }
    let v = project.viewpoints.remove(i);
    project.viewpoints.insert(p.index, v);
    Ok(Value::Null)
}
