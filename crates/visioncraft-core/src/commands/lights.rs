use super::{check_layer, fail, merge_update, next_id};
use crate::light::{Light, LightKind};
use crate::project::{LAYER_LIGHTING, Project};
use crate::session::{CommandError, Session, bad, parse};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    s.register(
        "light.add",
        "Add a light: {name, position, kind? (point, or spot with direction and cones), color_temperature_k? (2700), lumens? (800), range_m? (10), enabled?, layer?, source?}; returns {id}",
        light_add,
    );
    s.register(
        "light.update",
        "Change fields of a light: {id, name?, kind?, position?, color_temperature_k?, lumens?, range_m?, enabled?, layer?, source?}",
        light_update,
    );
    s.register("light.remove", "Remove a light: {id}", light_remove);
}

fn light_add(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    struct P {
        name: String,
        position: [f32; 3],
        #[serde(default = "point")]
        kind: LightKind,
        #[serde(default = "warm")]
        color_temperature_k: f32,
        #[serde(default = "lumens")]
        lumens: f32,
        #[serde(default = "range")]
        range_m: f32,
        #[serde(default = "yes")]
        enabled: bool,
        layer: Option<String>,
        source: Option<String>,
    }
    fn point() -> LightKind {
        LightKind::Point
    }
    fn warm() -> f32 {
        2700.0
    }
    fn lumens() -> f32 {
        800.0
    }
    fn range() -> f32 {
        10.0
    }
    fn yes() -> bool {
        true
    }
    let p: P = parse(params)?;
    let layer = p.layer.unwrap_or_else(|| LAYER_LIGHTING.into());
    check_layer(project, &layer)?;
    let id = next_id("li", |id| project.lights.iter().any(|l| l.id == id));
    let light = Light {
        id: id.clone(),
        name: p.name,
        layer,
        kind: p.kind,
        position: p.position,
        color_temperature_k: p.color_temperature_k,
        lumens: p.lumens,
        range_m: p.range_m,
        enabled: p.enabled,
        source: p.source,
    };
    light.validate().map_err(|e| bad(&e))?;
    project.lights.push(light);
    Ok(json!({ "id": id }))
}

fn light_update(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    let at = project
        .lights
        .iter()
        .position(|l| l.id == id)
        .ok_or_else(|| fail(format!("no light `{id}`")))?;
    let updated = merge_update(&project.lights[at], params)?;
    updated.validate().map_err(|e| bad(&e))?;
    check_layer(project, &updated.layer)?;
    project.lights[at] = updated;
    Ok(Value::Null)
}

fn light_remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let id = params["id"].as_str().ok_or_else(|| bad("missing id"))?;
    let before = project.lights.len();
    project.lights.retain(|l| l.id != id);
    if project.lights.len() == before {
        return Err(fail(format!("no light `{id}`")));
    }
    Ok(Value::Null)
}
