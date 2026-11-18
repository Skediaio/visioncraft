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
            "Adds a visible layer that models, shapes, generators and lights can sit on. With `context` the layer holds surroundings (a neighbour's house as a closed extrusion, an imported model): they cast and receive shadow but are not part of the design, draw in one neutral massing look and stay out of `query.quantities`.",
            vec![
                param("name", T::String, "Display name.").required(),
                param("context", T::Boolean, "True for a surroundings layer.").default(json!(false)),
            ],
            "{id}",
            json!({"name": "Neighbours", "context": true}),
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
            "layer.set_context",
            "Mark Layer as Surroundings",
            "Marks a layer as surroundings (true) or as part of the design (false). Everything on a context layer, such as a neighbour's house, still casts and receives shadow but draws in one neutral massing look whatever its materials, and `query.quantities` leaves it out. Plantings on it draw normally.",
            vec![
                param("id", T::String, "Layer id.").required(),
                param("context", T::Boolean, "True for surroundings.").required(),
            ],
            "null",
            json!({"id": "l1", "context": true}),
        ),
        layer_set_context,
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
            "Moves a model, shape, generator, light or underlay to another layer.",
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
            "Sets the season, snow, exposure, look, sky, clouds, years after planting, haze, wind, the ground plane, the surroundings, ground clutter and the view distance stored with the project; only the fields given change.",
            vec![
                param("season", T::String, "spring, summer, autumn or winter: the plants as they are then, and the project's date set to the season's day at the site (20 April, 21 June, 20 October, 15 January in the north; six months on in the south), the hour kept. An exact date (time.set) maps back to its season."),
                param("snow", T::Number, "Snow cover 0..1 on the ground, lawns, roofs, terraces, hedge tops, evergreen crowns, branches, wall tops and sills, not under roofs or dense evergreens; about 0.7 for a snowy winter. 0: none.").default(json!(0)),
                param("exposure_ev", T::Number, "Exposure offset in stops; null for automatic exposure."),
                param("sky", T::Object, "{type: \"physical\"} or {type: \"hdri\", id: string, rotation_deg: number, intensity: number >= 0}.")
                    .default(json!({"type": "physical"})),
                param("haze", T::Number, "Atmospheric haze as a turbidity: 1 pure air, 2.5 a clear day (about 30 km visibility), 6 hazy (9 km), 20 misty (2.5 km); 0 turns the haze off.").default(json!(2.5)),
                param("wind_speed_mps", T::Number, "Wind speed in m/s, 0 (perfectly still) to 60: plants lean with the drag (growing with the speed squared, saturating as crowns streamline), leaves flutter faster as it rises.").unit("m/s").default(json!(2.0)),
                param("wind_beaufort", T::Any, "A Beaufort step as a preset for `wind_speed_mps` (v = 0.836 B^1.5 m/s): 0-10 or its name. 0 calm (smoke rises), 1 light air, 2 light breeze (leaves rustle), 3 gentle breeze (leaves and twigs in constant motion), 4 moderate breeze (small branches move), 5 fresh breeze (small trees begin to sway), 6 strong breeze (large branches move), 7 near gale (whole trees move), 8 gale (twigs break off), 9 strong gale, 10 storm. Not together with `wind_speed_mps`."),
                param("wind_gustiness", T::Number, "How much the wind varies in gusts travelling downwind, 0 (steady) to 1.").default(json!(0.5)),
                param("wind_strength", T::Number, "Deprecated: the relative wind of older projects (1 is 6.5 m/s); use `wind_speed_mps`."),
                param("wind_direction_deg", T::Number, "Direction the wind blows towards, true bearing clockwise from north seen from above (drawing -Z unless site.set north_deg turns it).").unit("deg").default(json!(0)),
                param("ground", T::Object, "Ground plane {height: m, color: [r, g, b] linear}; null for none.")
                    .default(json!({"height": 0, "color": [0.065, 0.12, 0.035]})),
                param("clouds", T::Object, "The cloud layer over the analytic sky: {kind: \"clear\" | \"cumulus\" (light fair-weather clouds) | \"overcast\", cover: 0..1}; overcast softens shadows and dims the sun.").default(json!({"kind": "cumulus", "cover": 0.35})),
                param("lights", T::Boolean, "Lamps and lit rooms on (true) or off (false); null: on while the sun is below the horizon at the time shown."),
                param("clutter", T::Boolean, "Ground clutter: fallen leaves under deciduous trees (most in October and November), stones and twigs on soil and gravel, grass hanging over the lawn's edge. Off at LOW quality whatever this says.").default(json!(true)),
                param("years", T::Number, "Years after planting to show the plants at (0..60): each grows from its planting age along its species' curve; null shows everything full grown.").unit("years"),
                param("look", T::String, "neutral, natural or vivid: the picture's exposure, contrast and colour on top of the tone map.").default(json!("natural")),
                param("surroundings", T::String, "The land round the design on the endless ground, beyond its plot: fields (farmland with hedgerows, woods and distant hills), woodland, dunes, or none (a plain plane).").default(json!("fields")),
                param("view_distance", T::Number, "How far plants, shadows and a clear view reach, as a multiple of the default; 0.25 to 4.")
                    .unit("x")
                    .default(json!(1.0)),
            ],
            "null",
            json!({"haze": 3.5, "wind_speed_mps": 3.3, "ground": null, "view_distance": 2.0}),
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
        #[serde(default)]
        context: bool,
    }
    let p: P = parse(params)?;
    let id = next_id("l", |id| project.layers.iter().any(|l| l.id == id));
    project.layers.push(Layer {
        id: id.clone(),
        name: p.name,
        visible: true,
        context: p.context,
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

fn layer_set_context(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        context: bool,
    }
    let p: P = parse(params)?;
    layer(project, &p.id)?.context = p.context;
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
    // The season is the date's, not a view field: it moves the project's date.
    let mut params = params.clone();
    let season = match params.as_object_mut().and_then(|o| o.remove("season")) {
        None => None,
        Some(v) => Some(v.as_str().and_then(crate::Season::parse).ok_or_else(|| {
            crate::session::bad("`season` must be spring, summer, autumn or winter")
        })?),
    };
    // Presets and the deprecated relative strength become a speed.
    let mut speed = None;
    if let Some(o) = params.as_object_mut() {
        let beaufort = o.remove("wind_beaufort");
        let relative = o.remove("wind_strength");
        if beaufort.is_some() && (o.contains_key("wind_speed_mps") || relative.is_some()) {
            return Err(crate::session::bad(
                "`wind_beaufort` sets the speed: not together with `wind_speed_mps`",
            ));
        }
        if let Some(b) = beaufort {
            let force = crate::wind::parse_beaufort(&b).ok_or_else(|| {
                crate::session::bad("`wind_beaufort` must be 0 to 10 or a step's name (\"gale\")")
            })?;
            speed = Some(crate::wind::beaufort_mps(force));
        }
        if let Some(r) = relative {
            let r = r.as_f64().filter(|r| *r >= 0.0).ok_or_else(|| {
                crate::session::bad("`wind_strength` must be a number, 0 or more")
            })?;
            speed = Some((r as f32 * crate::wind::RELATIVE_MPS).min(crate::wind::MAX_SPEED_MPS));
        }
    }
    if let (Some(v), Some(o)) = (speed, params.as_object_mut()) {
        o.insert("wind_speed_mps".into(), json!(v));
    }
    // `view_distance` is not written while it is 1, `look` while natural, `clouds` while default.
    let view = super::merge_update(
        &project.view,
        &params,
        &[
            "view_distance",
            "look",
            "clouds",
            "years",
            "lights",
            "clutter",
            "snow",
            "surroundings",
        ],
    )?;
    view.validate().map_err(|e| crate::session::bad(&e))?;
    let time = season.map_or(project.time, |s| {
        s.time(project.time, project.site.latitude)
    });
    if view.snow > 0.0
        && crate::Season::of(time.month, project.site.latitude) != crate::Season::Winter
    {
        return Err(crate::session::bad(
            "snow lies only in winter: give `season: \"winter\"` with it, or `snow: 0` first",
        ));
    }
    project.view = view;
    project.time = time;
    Ok(Value::Null)
}
