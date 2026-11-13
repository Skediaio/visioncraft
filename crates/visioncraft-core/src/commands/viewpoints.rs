use super::{fail, merge_update, next_id};
use crate::project::{Project, SunTime};
use crate::session::{CommandError, Session, bad, parse};
use crate::spec::{ParamType as T, param, spec};
use crate::viewpoint::Viewpoint;
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    let time = || {
        param(
            "time",
            T::Object,
            "The sun to show from here {year, month, day, hour}; any part may be left out (the project's default fills in); null for the project's own time.",
        )
    };
    let fov = || param("fov_y_deg", T::Number, "Vertical field of view, 1..179.").unit("deg");
    s.register_with_spec(
        spec(
            "viewpoint.add",
            "Save Viewpoint",
            "Saves a camera position and target, optionally with a time of day.",
            vec![
                param("name", T::String, "Display name.").required(),
                param("position", T::Point3, "Camera position [x, y, z].").unit("m").required(),
                param("target", T::Point3, "Point it looks at; must differ from the position.").unit("m").required(),
                fov().default(json!(60)),
                time(),
            ],
            "{id} (v1, v2...)",
            json!({"name": "From the terrace", "position": [0, 1.6, 8], "target": [0, 1.6, 0], "time": {"hour": 19.5}}),
        ),
        viewpoint_add,
    );
    s.register_with_spec(
        spec(
            "viewpoint.update",
            "Update Viewpoint",
            "Changes fields of a viewpoint.",
            vec![
                param("id", T::String, "Viewpoint id.").required(),
                param("name", T::String, "Display name."),
                param("position", T::Point3, "Camera position.").unit("m"),
                param("target", T::Point3, "Look-at point.").unit("m"),
                fov(),
                time(),
            ],
            "null",
            json!({"id": "v1", "fov_y_deg": 75}),
        ),
        viewpoint_update,
    );
    s.register_with_spec(
        spec(
            "viewpoint.remove",
            "Remove Viewpoint",
            "Removes a viewpoint.",
            vec![param("id", T::String, "Viewpoint id.").required()],
            "null",
            json!({"id": "v2"}),
        ),
        viewpoint_remove,
    );
    s.register_with_spec(
        spec(
            "viewpoint.reorder",
            "Reorder Viewpoint",
            "Moves a viewpoint to a place in the list.",
            vec![
                param("id", T::String, "Viewpoint id.").required(),
                param("index", T::Integer, "New index, from 0, inside the list.").required(),
            ],
            "null",
            json!({"id": "v2", "index": 0}),
        ),
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
    #[serde(deny_unknown_fields)]
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
    #[serde(deny_unknown_fields)]
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
