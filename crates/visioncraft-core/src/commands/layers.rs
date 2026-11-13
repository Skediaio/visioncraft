use super::{fail, next_id};
use crate::project::{Layer, Project};
use crate::session::{CommandError, Session, parse};
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    s.register_with_spec(
        spec(
            "layer.add",
            "Add Layer",
            "Adds a visible layer that models, shapes, generators and lights can sit on.",
            vec![param("name", T::String, "Display name.").required()],
            "{id}",
            json!({"name": "Terrace"}),
        ),
        layer_add,
    );
    s.register_with_spec(
        spec(
            "layer.rename",
            "Rename Layer",
            "Renames a layer.",
            vec![
                param("id", T::String, "Layer id.").required(),
                param("name", T::String, "New display name.").required(),
            ],
            "null",
            json!({"id": "shapes", "name": "Drawn shapes"}),
        ),
        layer_rename,
    );
    s.register_with_spec(
        spec(
            "layer.set_visible",
            "Show or Hide Layer",
            "Shows or hides everything on a layer.",
            vec![
                param("id", T::String, "Layer id.").required(),
                param("visible", T::Boolean, "True to show.").required(),
            ],
            "null",
            json!({"id": "planting", "visible": false}),
        ),
        layer_set_visible,
    );
    s.register_with_spec(
        spec(
            "layer.remove",
            "Remove Layer",
            "Removes a layer; refused while any model, shape, generator or light sits on it.",
            vec![param("id", T::String, "Layer id.").required()],
            "null",
            json!({"id": "l1"}),
        ),
        layer_remove,
    );
    s.register_with_spec(
        spec(
            "layer.assign",
            "Move to Layer",
            "Moves a model, shape, generator or light to another layer.",
            vec![
                param(
                    "item",
                    T::String,
                    "Id of the model, shape, generator or light.",
                )
                .required(),
                param("layer", T::String, "Id of the layer to move it to.").required(),
            ],
            "null",
            json!({"item": "S1", "layer": "l1"}),
        ),
        layer_assign,
    );
    s.register_with_spec(
        spec(
            "style.set",
            "Set Render Style",
            "Switches the presentation mode that draws every surface white (clay).",
            vec![param("clay", T::Boolean, "True for clay mode.").required()],
            "null",
            json!({"clay": true}),
        ),
        style_set,
    );
    s.register_with_spec(
        spec(
            "view.set",
            "Set View",
            "Sets exposure, sky, haze, wind and the ground plane stored with the project; only the fields given change.",
            vec![
                param("exposure_ev", T::Number, "Exposure offset in stops; null for automatic exposure."),
                param("sky", T::Object, "{type: \"physical\"} or {type: \"hdri\", id: string, rotation_deg: number, intensity: number >= 0}.")
                    .default(json!({"type": "physical"})),
                param("haze", T::Number, "Atmospheric haze (turbidity), 0 or more.").default(json!(2.5)),
                param("wind_strength", T::Number, "Wind for plants, 0 or more.").default(json!(0.3)),
                param("wind_direction_deg", T::Number, "Direction the wind blows towards, clockwise from north seen from above.").unit("deg").default(json!(0)),
                param("ground", T::Object, "Ground plane {height: m, color: [r, g, b] linear}; null for none.")
                    .default(json!({"height": 0, "color": [0.10, 0.17, 0.08]})),
            ],
            "null",
            json!({"haze": 3.5, "wind_strength": 0.5, "ground": null}),
        ),
        view_set,
    );
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
    #[serde(deny_unknown_fields)]
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
    #[serde(deny_unknown_fields)]
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
    #[serde(deny_unknown_fields)]
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
    #[serde(deny_unknown_fields)]
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
    #[serde(deny_unknown_fields)]
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
    #[serde(deny_unknown_fields)]
    struct P {
        clay: bool,
    }
    project.style.clay = parse::<P>(params)?.clay;
    Ok(Value::Null)
}

fn view_set(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let view = super::merge_update(&project.view, params, &[])?;
    view.validate().map_err(|e| crate::session::bad(&e))?;
    project.view = view;
    Ok(Value::Null)
}
