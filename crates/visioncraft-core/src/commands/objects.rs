use super::{fail, next_id};
use crate::object::UserObject;
use crate::project::Project;
use crate::session::{CommandError, Session, bad, parse};
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    s.register_with_spec(
        spec(
            "object.add",
            "Add Object",
            "Registers one of the user's own models (.glb) so it can be placed as the asset `user:<id>`.",
            vec![
                param("name", T::String, "Display name.").required(),
                param("file", T::String, "Path relative to the project file, or `objects/<id>.glb` inside a bundle.").required(),
                param("size_m", T::Point3, "Width, height, depth of the model as it stands, each above zero.").unit("m").required(),
            ],
            "{id} (uo1, uo2...); place it with the asset id `user:<id>`",
            json!({"name": "Garden bench", "file": "objects/bench.glb", "size_m": [1.8, 0.9, 0.6]}),
        ),
        add,
    );
    s.register_with_spec(
        spec(
            "object.remove",
            "Remove Object",
            "Removes a user object; refused while a placement, palette or generator uses it (the error names them).",
            vec![param("id", T::String, "Object id.").required()],
            "null",
            json!({"id": "uo1"}),
        ),
        remove,
    );
    s.register_with_spec(
        spec(
            "object.rename",
            "Rename Object",
            "Renames a user object.",
            vec![
                param("id", T::String, "Object id.").required(),
                param("name", T::String, "New display name.").required(),
            ],
            "null",
            json!({"id": "uo1", "name": "Bench"}),
        ),
        rename,
    );
    s.register_with_spec(
        spec(
            "object.list",
            "List Objects",
            "Lists the user objects with the generators and placements that use each.",
            vec![],
            "[{id, name, file, size_m, asset (the `user:<id>` asset id), used_by: [generator id]}]",
            json!({}),
        )
        .read_only(),
        list,
    );
}

fn add(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: String,
        file: String,
        size_m: [f32; 3],
    }
    let p: P = parse(params)?;
    let id = next_id("uo", |id| project.objects.iter().any(|o| o.id == id));
    let object = UserObject {
        id: id.clone(),
        name: p.name,
        file: p.file,
        size_m: p.size_m,
        extensions: Default::default(),
    };
    object.validate().map_err(|e| bad(&e))?;
    project.objects.push(object);
    Ok(json!({ "id": id }))
}

fn users(project: &Project, id: &str) -> Vec<String> {
    let asset = format!("user:{id}");
    project
        .generators
        .iter()
        .filter(|g| g.uses_asset(&asset))
        .map(|g| g.id().to_string())
        .collect()
}

fn find<'a>(project: &'a mut Project, params: &Value) -> Result<&'a mut UserObject, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    project
        .objects
        .iter_mut()
        .find(|o| o.id == id)
        .ok_or_else(|| fail(format!("no object `{id}`")))
}

fn remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = find(project, params)?.id.clone();
    let used = users(project, &id);
    if !used.is_empty() {
        return Err(fail(format!(
            "object `{id}` is used by {}",
            used.join(", ")
        )));
    }
    project.objects.retain(|o| o.id != id);
    Ok(Value::Null)
}

fn rename(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let name = params["name"]
        .as_str()
        .ok_or_else(|| bad("missing name"))?
        .to_string();
    if name.trim().is_empty() {
        return Err(bad("a name cannot be empty"));
    }
    find(project, params)?.name = name;
    Ok(Value::Null)
}

fn list(project: &mut Project, _: &Value) -> Result<Value, CommandError> {
    Ok(Value::Array(
        project
            .objects
            .iter()
            .map(|o| {
                json!({"id": o.id, "name": o.name, "file": o.file, "size_m": o.size_m,
                       "asset": o.asset_id(), "used_by": users(project, &o.id)})
            })
            .collect(),
    ))
}
