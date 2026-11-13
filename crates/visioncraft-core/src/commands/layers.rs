use super::{fail, next_id};
use crate::project::{Layer, Project};
use crate::session::{CommandError, Session, parse};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    s.register(
        "layer.add",
        "Add a visible layer: {name}; returns {id}",
        layer_add,
    );
    s.register("layer.rename", "Rename a layer: {id, name}", layer_rename);
    s.register(
        "layer.set_visible",
        "Show or hide a layer: {id, visible}",
        layer_set_visible,
    );
    s.register(
        "layer.remove",
        "Remove a layer nothing sits on: {id}",
        layer_remove,
    );
    s.register(
        "layer.assign",
        "Move a model, shape, generator or light to a layer: {item, layer}",
        layer_assign,
    );
    s.register("style.set", "Set the render style: {clay}", style_set);
}

fn layer<'a>(project: &'a mut Project, id: &str) -> Result<&'a mut Layer, CommandError> {
    project
        .layers
        .iter_mut()
        .find(|l| l.id == id)
        .ok_or_else(|| fail(format!("no layer `{id}`")))
}

fn layer_add(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        name: String,
    }
    let p: P = parse(params)?;
    let id = next_id("l", |id| project.layers.iter().any(|l| l.id == id));
    project.layers.push(Layer {
        id: id.clone(),
        name: p.name,
        visible: true,
    });
    Ok(json!({ "id": id }))
}

fn layer_rename(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        id: String,
        name: String,
    }
    let p: P = parse(params)?;
    layer(project, &p.id)?.name = p.name;
    Ok(Value::Null)
}

fn layer_set_visible(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        id: String,
        visible: bool,
    }
    let p: P = parse(params)?;
    layer(project, &p.id)?.visible = p.visible;
    Ok(Value::Null)
}

fn layer_remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        id: String,
    }
    let p: P = parse(params)?;
    layer(project, &p.id)?;
    if project.layer_in_use(&p.id) {
        return Err(fail(format!("layer `{}` still has items on it", p.id)));
    }
    project.layers.retain(|l| l.id != p.id);
    Ok(Value::Null)
}

fn layer_assign(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        item: String,
        layer: String,
    }
    let p: P = parse(params)?;
    super::check_layer(project, &p.layer)?;
    *project
        .layer_of_mut(&p.item)
        .ok_or_else(|| fail(format!("no model, shape, generator or light `{}`", p.item)))? =
        p.layer;
    Ok(Value::Null)
}

fn style_set(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        clay: bool,
    }
    project.style.clay = parse::<P>(params)?.clay;
    Ok(Value::Null)
}
