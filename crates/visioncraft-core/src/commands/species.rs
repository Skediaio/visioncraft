use super::{fail, next_id};
use crate::project::{CustomSpecies, Project};
use crate::session::{CommandError, Session, bad, parse};
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    let params = || {
        param(
            "params",
            T::Any,
            "The plant generator's parameters as JSON; the core does not read them.",
        )
    };
    s.register_with_spec(
        spec(
            "species.add",
            "Add Custom Species",
            "Adds a custom plant species, usable as an asset id in any palette.",
            vec![
                param("name", T::String, "Display name.").required(),
                param(
                    "base",
                    T::String,
                    "Built-in species id it was derived from.",
                ),
                params().required(),
            ],
            "{id} (sp1, sp2...)",
            json!({"name": "Tall birch", "base": "betula-pendula", "params": {"height": 14.0}}),
        ),
        add,
    );
    s.register_with_spec(
        spec(
            "species.update",
            "Update Custom Species",
            "Changes the name or parameters of a custom species.",
            vec![
                param("id", T::String, "Species id.").required(),
                param("name", T::String, "Display name."),
                params(),
            ],
            "null",
            json!({"id": "sp1", "name": "Taller birch"}),
        ),
        update,
    );
    s.register_with_spec(
        spec(
            "species.remove",
            "Remove Custom Species",
            "Removes a custom species; refused while a generator uses it.",
            vec![param("id", T::String, "Species id.").required()],
            "null",
            json!({"id": "sp1"}),
        ),
        remove,
    );
}

fn add(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
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
    #[serde(deny_unknown_fields)]
    struct P {
        #[serde(rename = "id")]
        _id: String,
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
