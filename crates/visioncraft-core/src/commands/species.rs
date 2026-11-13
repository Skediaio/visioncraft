use super::{fail, next_id};
use crate::project::{CustomSpecies, Project};
use crate::session::{CommandError, Session, bad, parse};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    s.register(
        "species.add",
        "Add a custom plant species: {name, base? (built-in species id), params (plant generator JSON)}; returns {id}",
        add,
    );
    s.register(
        "species.update",
        "Change a custom species: {id, name?, params?}",
        update,
    );
    s.register(
        "species.remove",
        "Remove a custom species no generator uses: {id}",
        remove,
    );
}

fn add(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        name: String,
        base: Option<String>,
        params: Value,
    }
    let p: P = parse(params)?;
    let id = next_id("sp", |id| project.species.iter().any(|s| s.id == id));
    project.species.push(CustomSpecies {
        id: id.clone(),
        name: p.name,
        base: p.base,
        params: p.params,
        extensions: Default::default(),
    });
    Ok(json!({ "id": id }))
}

fn find<'a>(
    project: &'a mut Project,
    params: &Value,
) -> Result<&'a mut CustomSpecies, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    project
        .species
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| fail(format!("no species `{id}`")))
}

fn update(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        name: Option<String>,
        params: Option<Value>,
    }
    let p: P = parse(params)?;
    let s = find(project, params)?;
    s.name = p.name.unwrap_or_else(|| std::mem::take(&mut s.name));
    s.params = p.params.unwrap_or_else(|| std::mem::take(&mut s.params));
    Ok(Value::Null)
}

fn remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = find(project, params)?.id.clone();
    if project.generators.iter().any(|g| g.uses_asset(&id)) {
        return Err(fail(format!("species `{id}` is still used by a generator")));
    }
    project.species.retain(|s| s.id != id);
    Ok(Value::Null)
}
