use super::{check_layer, fail, merge_update, next_id};
use crate::light::{Light, LightKind};
use crate::project::{LAYER_LIGHTING, Project};
use crate::session::{CommandError, Session, bad, parse};
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    let layer = || param("layer", T::String, "Layer id.");
    let kind = || {
        param(
            "kind",
            T::Object,
            "{type: \"point\"} or {type: \"spot\", direction: [x, y, z] (where it points), inner_cone_deg, outer_cone_deg} with 0 <= inner <= outer <= 90.",
        )
    };
    let kelvin = || {
        param(
            "color_temperature_k",
            T::Number,
            "Colour temperature, 1000..40000 (2700 warm white, 6500 daylight).",
        )
        .unit("K")
    };
    let lumens = || param("lumens", T::Number, "Luminous flux, 0 or more.").unit("lm");
    let range = || {
        param(
            "range_m",
            T::Number,
            "Distance at which the light is cut off, above zero.",
        )
        .unit("m")
    };
    let source = || {
        param(
            "source",
            T::String,
            "Id of the placement or generator whose lamp asset made this light, so moving or removing the lamp can update it.",
        )
    };
    s.register_with_spec(
        spec(
            "light.add",
            "Add Light",
            "Adds a point or spot light.",
            vec![
                param("name", T::String, "Display name.").required(),
                param("position", T::Point3, "[x, y, z] of the emitter.").unit("m").required(),
                kind().default(json!({"type": "point"})),
                kelvin().default(json!(2700)),
                lumens().default(json!(800)),
                range().default(json!(10)),
                param("enabled", T::Boolean, "Whether it shines.").default(json!(true)),
                layer().default(json!("lighting")),
                source(),
            ],
            "{id} (li1, li2...)",
            json!({"name": "Garden spot", "position": [3, 2.5, 1], "kind": {"type": "spot", "direction": [0, -1, 0], "inner_cone_deg": 20, "outer_cone_deg": 40}, "lumens": 1200}),
        ),
        light_add,
    );
    s.register_with_spec(
        spec(
            "light.update",
            "Update Light",
            "Changes fields of a light.",
            vec![
                param("id", T::String, "Light id.").required(),
                param("name", T::String, "Display name."),
                kind(),
                param("position", T::Point3, "[x, y, z] of the emitter.").unit("m"),
                kelvin(),
                lumens(),
                range(),
                param("enabled", T::Boolean, "Whether it shines."),
                layer(),
                source(),
            ],
            "null",
            json!({"id": "li1", "lumens": 1500, "enabled": false}),
        ),
        light_update,
    );
    s.register_with_spec(
        spec(
            "light.remove",
            "Remove Light",
            "Removes a light.",
            vec![param("id", T::String, "Light id.").required()],
            "null",
            json!({"id": "li1"}),
        ),
        light_remove,
    );
}

fn light_add(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
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
